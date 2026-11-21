// Runs inside hiruzen's embedded JavaScript engine. Provides the small part
// of Fig's generator API that completion specs rely on.
//
// The host supplies one function, __hz_exec(json), which runs a command and
// returns {"stdout", "stderr", "status"} as JSON. Nothing else reaches outside
// the engine: there is no file, network or timer access.
globalThis.__hz_specs = Object.create(null);
globalThis.console = { log() {}, warn() {}, error() {}, info() {}, debug() {} };
// Specs occasionally defer work with a timer; run it once the current job ends.
globalThis.setTimeout = (callback, _delay, ...args) => {
  Promise.resolve().then(() => callback(...args));
  return 0;
};
globalThis.clearTimeout = () => {};

globalThis.__hz_run = async (key, path, tokens, cwd, env) => {
  let generator = __hz_specs[key];
  for (const step of path) {
    if (generator == null) return "null";
    generator = generator[step];
  }
  if (generator == null) return "null";

  const execute = async (input) => {
    // Older specs pass a shell command line as a string.
    const request =
      typeof input === "string"
        ? { shell: input, cwd }
        : { command: input.command, args: input.args ?? [], cwd: input.cwd ?? cwd, env: input.env };
    return JSON.parse(__hz_exec(JSON.stringify(request)));
  };
  const token = tokens[tokens.length - 1] ?? "";
  const context = {
    currentWorkingDirectory: cwd,
    environmentVariables: env,
    currentProcess: "",
    sshPrefix: "",
    isDangerous: false,
    searchTerm: token,
  };

  let suggestions = [];
  if (generator.script != null) {
    const script = typeof generator.script === "function" ? generator.script(tokens) : generator.script;
    if (script != null) {
      const result = await execute(Array.isArray(script) ? { command: script[0], args: script.slice(1) } : script);
      const stdout = (result.stdout ?? "").trim();
      if (typeof generator.postProcess === "function") {
        suggestions.push(...(generator.postProcess(stdout, tokens) ?? []));
      } else if (typeof generator.splitOn === "string") {
        suggestions.push(...stdout.split(generator.splitOn).map((name) => ({ name })));
      }
    }
  }
  if (typeof generator.custom === "function") {
    suggestions.push(...((await generator.custom(tokens, execute, context)) ?? []));
  }

  let query = null;
  if (typeof generator.getQueryTerm === "function") {
    query = generator.getQueryTerm(token);
  } else if (typeof generator.getQueryTerm === "string") {
    const at = token.lastIndexOf(generator.getQueryTerm);
    query = at === -1 ? token : token.slice(at + generator.getQueryTerm.length);
  }

  const items = [];
  for (const suggestion of suggestions) {
    if (suggestion == null) continue;
    if (typeof suggestion === "string") {
      items.push({ name: suggestion });
      continue;
    }
    const { name, description, insertValue, displayName, priority, hidden, type } = suggestion;
    items.push({ name, description, insertValue, displayName, priority, hidden, type });
  }
  return JSON.stringify({ query: typeof query === "string" ? query : null, items });
};
