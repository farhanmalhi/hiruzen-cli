//! Runs the JavaScript that completion specs use to compute suggestions.
//!
//! Many Fig specs describe a dynamic argument with a function: it parses a
//! command's output, or decides which command to run from what has been typed.
//! Those functions run here, in an embedded engine with no access to the
//! machine except through the `exec` callback the caller provides.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex, OnceLock};

use boa_engine::builtins::promise::PromiseState;
use boa_engine::object::builtins::JsPromise;
use boa_engine::{Context, JsResult, JsString, JsValue, NativeFunction, Source};
use serde::Deserialize;

const SHIM: &str = include_str!("shim.js");
/// Loop iterations one generator may run before it is stopped, so a faulty
/// spec cannot hang suggestions.
const LOOP_LIMIT: u64 = 5_000_000;
/// Stack for the engine thread. Only the part actually used is ever backed
/// by memory.
const ENGINE_STACK: usize = 256 * 1024 * 1024;

/// A command a generator asks to run.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ExecRequest {
    pub command: Option<String>,
    pub args: Vec<String>,
    /// A shell command line, used by older specs instead of `command`.
    pub shell: Option<String>,
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, Option<String>>>,
}

/// Runs a command for a generator and returns `{"stdout","stderr","status"}` as JSON.
pub type Exec<'a> = &'a (dyn Fn(ExecRequest) -> String + Sync);
/// Supplies a spec's JavaScript module.
pub type Module<'a> = &'a (dyn Fn() -> Option<String> + Sync);

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Output {
    /// The part of the typed word the suggestions complete, when the
    /// generator narrows it (for example the text after the last comma).
    pub query: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Item {
    pub name: serde_json::Value,
    pub description: serde_json::Value,
    pub insert_value: serde_json::Value,
    pub display_name: serde_json::Value,
    pub priority: serde_json::Value,
    pub hidden: serde_json::Value,
    #[serde(rename = "type")]
    pub kind: serde_json::Value,
}

struct Runtime {
    context: Context,
    /// Spec modules evaluated so far, and whether each loaded.
    loaded: HashMap<String, bool>,
}

type ModulePtr<'a> = *const (dyn Fn() -> Option<String> + Sync + 'a);
type ExecPtr<'a> = *const (dyn Fn(ExecRequest) -> String + Sync + 'a);

/// One generator to run, handed to the engine thread.
struct Job {
    key: String,
    path: String,
    tokens: Vec<String>,
    cwd: String,
    env: HashMap<String, String>,
    module: ModulePtr<'static>,
    exec: ExecPtr<'static>,
    reply: mpsc::Sender<Option<Output>>,
}

// SAFETY: the pointers refer to `Sync` closures, and `run` keeps them alive
// by blocking until the engine thread has replied or gone away.
unsafe impl Send for Job {}

thread_local! {
    // The callback for the generator currently running on the engine thread.
    static EXEC: RefCell<Option<ExecPtr<'static>>> = const { RefCell::new(None) };
}

/// `__hz_exec(json)`: the only way out of the engine.
fn host_exec(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let failed = || JsValue::from(JsString::from(r#"{"stdout":"","stderr":"","status":1}"#));
    let Some(input) = args.first() else { return Ok(failed()) };
    let input = input.to_string(context)?.to_std_string_escaped();
    let Ok(request) = serde_json::from_str::<ExecRequest>(&input) else {
        return Ok(failed());
    };
    let pointer = EXEC.with(|exec| *exec.borrow());
    let Some(pointer) = pointer else { return Ok(failed()) };
    // SAFETY: set by `execute` for the job being run, whose caller is blocked
    // in `run` keeping the closure alive, and cleared when the job ends.
    let output = unsafe { (*pointer)(request) };
    Ok(JsValue::from(JsString::from(output.as_str())))
}

fn new_runtime() -> Option<Runtime> {
    let mut context = Context::default();
    context.runtime_limits_mut().set_loop_iteration_limit(LOOP_LIMIT);
    context
        .register_global_callable(JsString::from("__hz_exec"), 1, NativeFunction::from_fn_ptr(host_exec))
        .ok()?;
    context.eval(Source::from_bytes(SHIM.as_bytes())).ok()?;
    Some(Runtime {
        context,
        loaded: HashMap::new(),
    })
}

/// Clears the callback when a job ends, however it ends.
struct ExecGuard;

impl Drop for ExecGuard {
    fn drop(&mut self) {
        EXEC.with(|exec| *exec.borrow_mut() = None);
    }
}

fn execute(runtime: &mut Runtime, job: &Job) -> Option<Output> {
    let loaded = match runtime.loaded.get(&job.key) {
        Some(loaded) => *loaded,
        None => {
            // SAFETY: see `Job`.
            let source = unsafe { (*job.module)() };
            let loaded = source.is_some_and(|source| {
                let script = format!("__hz_specs[{}] = {source};", serde_json::Value::from(job.key.as_str()));
                runtime.context.eval(Source::from_bytes(script.as_bytes())).is_ok()
            });
            runtime.loaded.insert(job.key.clone(), loaded);
            loaded
        }
    };
    if !loaded {
        return None;
    }

    // JSON is valid JavaScript, so the arguments are passed as literals.
    let call = format!(
        "__hz_run({}, {}, {}, {}, {})",
        serde_json::Value::from(job.key.as_str()),
        job.path,
        serde_json::to_string(&job.tokens).ok()?,
        serde_json::Value::from(job.cwd.as_str()),
        serde_json::to_string(&job.env).ok()?,
    );

    EXEC.with(|exec| *exec.borrow_mut() = Some(job.exec));
    let _guard = ExecGuard;

    let promise = runtime.context.eval(Source::from_bytes(call.as_bytes())).ok()?;
    let _ = runtime.context.run_jobs();
    let promise = JsPromise::from_object(promise.as_object()?).ok()?;
    let PromiseState::Fulfilled(value) = promise.state() else {
        return None;
    };
    let json = value.as_string()?.to_std_string_escaped();
    serde_json::from_str::<Option<Output>>(&json).ok()?
}

/// The engine runs on one thread of its own for the life of the process.
///
/// It needs a far deeper stack than a normal thread has, because parsing a
/// large spec module recurses deeply. Keeping it on one thread also means
/// each module is evaluated once, and the engine is never torn down (its
/// garbage collector does not survive being dropped at thread exit).
fn engine_thread() -> Option<&'static Mutex<mpsc::Sender<Job>>> {
    static ENGINE: OnceLock<Option<Mutex<mpsc::Sender<Job>>>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let (jobs, inbox) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("hiruzen-js".to_string())
                .stack_size(ENGINE_STACK)
                .spawn(move || {
                    let mut runtime: Option<Runtime> = None;
                    for job in inbox {
                        if runtime.is_none() {
                            runtime = new_runtime();
                        }
                        let output = runtime.as_mut().and_then(|runtime| execute(runtime, &job));
                        let _ = job.reply.send(output);
                    }
                })
                .ok()
                .map(|_| Mutex::new(jobs))
        })
        .as_ref()
}

/// Runs the generator at `path` inside the spec `key`.
///
/// `module` supplies the spec's JavaScript the first time it is needed.
/// Returns `None` when the module or generator cannot be run, which callers
/// treat as "no dynamic suggestions" and not as an error to show.
pub fn run(
    key: &str,
    module: Module<'_>,
    path: &serde_json::Value,
    tokens: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
    exec: Exec<'_>,
) -> Option<Output> {
    let (reply, result) = mpsc::channel();
    // SAFETY: erases the borrows' lifetimes so they can cross to the engine
    // thread. This function does not return until the engine has replied or
    // dropped the job, so the closures outlive every use of the pointers.
    let module: ModulePtr<'_> = module;
    let exec: ExecPtr<'_> = exec;
    let job = Job {
        key: key.to_string(),
        path: path.to_string(),
        tokens: tokens.to_vec(),
        cwd: cwd.to_string(),
        env: env.clone(),
        module: unsafe { std::mem::transmute::<ModulePtr<'_>, ModulePtr<'static>>(module) },
        exec: unsafe { std::mem::transmute::<ExecPtr<'_>, ExecPtr<'static>>(exec) },
        reply,
    };
    engine_thread()?.lock().ok()?.send(job).ok()?;
    // An error here means the engine thread died; there is nothing to suggest.
    result.recv().ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec_table(request: ExecRequest) -> String {
        let line = match (&request.command, &request.shell) {
            (Some(command), _) => std::iter::once(command.clone())
                .chain(request.args.clone())
                .collect::<Vec<_>>()
                .join(" "),
            (None, Some(shell)) => shell.clone(),
            _ => String::new(),
        };
        let stdout = match line.as_str() {
            "list things" => "alpha one\nbeta two\n",
            "list alpha" => "inner\n",
            _ => "",
        };
        serde_json::json!({ "stdout": stdout, "stderr": "", "status": 0 }).to_string()
    }

    const MODULE: &str = r#"(function(){
        const parse = (out) => out.split("\n").map((line) => { const [name, description] = line.split(" "); return { name, description }; });
        return { name: "tool", args: [
            { generators: { script: ["list", "things"], postProcess: parse } },
            { generators: [{ custom: async (tokens, exec) => { const r = await exec({ command: "list", args: [tokens[1]] }); return r.stdout.trim().split("\n").map((name) => ({ name, priority: 80 })); } }] },
            { generators: { script: (tokens) => ["list", "things"], splitOn: "\n", getQueryTerm: "," } },
            { generators: { custom: async () => { while (true) {} } } },
        ] };
    })()"#;

    fn run_path(key: &str, path: serde_json::Value, tokens: &[&str]) -> Option<Output> {
        let tokens: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
        run(
            key,
            &|| Some(MODULE.to_string()),
            &path,
            &tokens,
            "/tmp",
            &HashMap::new(),
            &exec_table,
        )
    }

    #[test]
    fn runs_post_process_on_command_output() {
        let out = run_path("tool", serde_json::json!(["args", 0, "generators"]), &["tool", ""]).unwrap();
        assert_eq!(out.items.len(), 2);
        assert_eq!(out.items[0].name, "alpha");
        assert_eq!(out.items[1].description, "two");
    }

    #[test]
    fn runs_async_custom_generators_that_call_back_for_commands() {
        let out = run_path(
            "tool",
            serde_json::json!(["args", 1, "generators", 0]),
            &["tool", "alpha", ""],
        )
        .unwrap();
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].name, "inner");
        assert_eq!(out.items[0].priority, 80);
    }

    #[test]
    fn reports_the_query_term() {
        let out = run_path(
            "tool",
            serde_json::json!(["args", 2, "generators"]),
            &["tool", "a", "b", "x,al"],
        )
        .unwrap();
        assert_eq!(out.query.as_deref(), Some("al"));
        assert_eq!(out.items.len(), 2);
    }

    #[test]
    fn stops_runaway_generators_and_bad_paths() {
        assert!(run_path("tool", serde_json::json!(["args", 3, "generators"]), &["tool"]).is_none());
        assert!(run_path("tool", serde_json::json!(["args", 9, "generators"]), &["tool"]).is_none());
        // The engine still works afterwards.
        assert!(run_path("tool", serde_json::json!(["args", 0, "generators"]), &["tool", ""]).is_some());
    }

    #[test]
    fn a_module_that_fails_to_load_yields_nothing() {
        let tokens = vec!["x".to_string()];
        let out = run(
            "broken",
            &|| Some("(function(){ syntax error".to_string()),
            &serde_json::json!([]),
            &tokens,
            "/tmp",
            &HashMap::new(),
            &exec_table,
        );
        assert!(out.is_none());
    }
}
