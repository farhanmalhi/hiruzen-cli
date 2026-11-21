//! The completion spec library: an embedded archive of Fig-format specs
//! (converted to JSON by `tools/specgen`) and the model they deserialize into.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, OnceLock};

use serde::de::{self, DeserializeOwned, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;

static ARCHIVE: &[u8] = include_bytes!("../specs/specs.bin");
const MAGIC: &[u8] = b"HZSP1\0";
/// hiruzen's own spec, written by hand; the Fig library has none for it.
const OWN_SPEC: &str = include_str!("../specs/hiruzen.json");

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Subcommand {
    #[serde(deserialize_with = "one_or_many")]
    pub name: Vec<String>,
    #[serde(deserialize_with = "lenient_string")]
    pub description: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub subcommands: Vec<Subcommand>,
    #[serde(deserialize_with = "lenient_vec")]
    pub options: Vec<Opt>,
    #[serde(deserialize_with = "one_or_many")]
    pub args: Vec<Arg>,
    #[serde(deserialize_with = "lenient_string")]
    pub load_spec: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub additional_suggestions: Vec<SpecSuggestion>,
    #[serde(deserialize_with = "lenient_number")]
    pub priority: Option<f64>,
    #[serde(deserialize_with = "lenient_bool")]
    pub hidden: bool,
    #[serde(deserialize_with = "lenient_string")]
    pub insert_value: Option<String>,
    pub parser_directives: Option<ParserDirectives>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ParserDirectives {
    #[serde(deserialize_with = "lenient_bool")]
    pub flags_are_posix_noncompliant: bool,
    #[serde(deserialize_with = "lenient_bool")]
    pub options_must_precede_arguments: bool,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Opt {
    #[serde(deserialize_with = "one_or_many")]
    pub name: Vec<String>,
    #[serde(deserialize_with = "lenient_string")]
    pub description: Option<String>,
    #[serde(deserialize_with = "one_or_many")]
    pub args: Vec<Arg>,
    #[serde(deserialize_with = "lenient_bool")]
    pub is_persistent: bool,
    #[serde(deserialize_with = "lenient_bool")]
    pub is_repeatable: bool,
    #[serde(deserialize_with = "lenient_vec")]
    pub exclusive_on: Vec<String>,
    #[serde(deserialize_with = "lenient_number")]
    pub priority: Option<f64>,
    #[serde(deserialize_with = "lenient_bool")]
    pub hidden: bool,
    #[serde(deserialize_with = "lenient_string")]
    pub insert_value: Option<String>,
    /// `true` or a separator string: the argument must be attached (`--opt=value`).
    #[serde(deserialize_with = "lenient_separator")]
    pub requires_separator: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Arg {
    #[serde(deserialize_with = "lenient_string")]
    pub name: Option<String>,
    #[serde(deserialize_with = "lenient_string")]
    pub description: Option<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub suggestions: Vec<SpecSuggestion>,
    #[serde(deserialize_with = "one_or_many")]
    pub template: Vec<String>,
    #[serde(deserialize_with = "lenient_vec")]
    pub generators: Vec<Generator>,
    #[serde(deserialize_with = "lenient_bool")]
    pub is_optional: bool,
    #[serde(deserialize_with = "lenient_bool")]
    pub is_variadic: bool,
    #[serde(deserialize_with = "lenient_bool")]
    pub is_command: bool,
    #[serde(deserialize_with = "lenient_string")]
    pub load_spec: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Generator {
    #[serde(deserialize_with = "one_or_many")]
    pub template: Vec<String>,
    /// The command to run, as argv. A string script is run through the shell.
    #[serde(deserialize_with = "one_or_many")]
    pub script: Vec<String>,
    #[serde(deserialize_with = "lenient_string")]
    pub split_on: Option<String>,
    /// Name of a hiruzen-native implementation, e.g. `git:localBranches`.
    #[serde(deserialize_with = "lenient_string")]
    pub native: Option<String>,
    /// The original generator needed a JavaScript function to produce output.
    #[serde(deserialize_with = "lenient_bool")]
    pub dynamic: bool,
    /// Where the generator sits inside the spec's JavaScript module, when
    /// producing its suggestions means running that module's code.
    pub js: Option<serde_json::Value>,
    /// The separator after which the typed word is completed, as in `a,b,c`.
    #[serde(deserialize_with = "lenient_string")]
    pub get_query_term: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct SpecSuggestion {
    pub name: Vec<String>,
    pub description: Option<String>,
    pub insert_value: Option<String>,
    pub priority: Option<f64>,
    pub hidden: bool,
}

impl<'de> Deserialize<'de> for SpecSuggestion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(rename_all = "camelCase", default)]
        struct Full {
            #[serde(deserialize_with = "one_or_many")]
            name: Vec<String>,
            #[serde(deserialize_with = "lenient_string")]
            description: Option<String>,
            #[serde(deserialize_with = "lenient_string")]
            insert_value: Option<String>,
            #[serde(deserialize_with = "lenient_number")]
            priority: Option<f64>,
            #[serde(deserialize_with = "lenient_bool")]
            hidden: bool,
        }

        struct SuggestionVisitor;
        impl<'de> Visitor<'de> for SuggestionVisitor {
            type Value = SpecSuggestion;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a suggestion string or object")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<SpecSuggestion, E> {
                Ok(SpecSuggestion {
                    name: vec![v.to_string()],
                    ..Default::default()
                })
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<SpecSuggestion, A::Error> {
                let full = Full::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(SpecSuggestion {
                    name: full.name,
                    description: full.description,
                    insert_value: full.insert_value,
                    priority: full.priority,
                    hidden: full.hidden,
                })
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<SpecSuggestion, E> {
                Ok(SpecSuggestion::default())
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<SpecSuggestion, E> {
                Ok(SpecSuggestion {
                    name: vec![v.to_string()],
                    ..Default::default()
                })
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<SpecSuggestion, E> {
                Ok(SpecSuggestion {
                    name: vec![v.to_string()],
                    ..Default::default()
                })
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<SpecSuggestion, E> {
                Ok(SpecSuggestion {
                    name: vec![v.to_string()],
                    ..Default::default()
                })
            }
            fn visit_unit<E: de::Error>(self) -> Result<SpecSuggestion, E> {
                Ok(SpecSuggestion::default())
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SpecSuggestion, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(SpecSuggestion::default())
            }
        }
        deserializer.deserialize_any(SuggestionVisitor)
    }
}

// --- Lenient field deserializers -------------------------------------------
// Specs are community-written JavaScript, so a field occasionally holds an
// unexpected type. A surprise in one field must not discard a whole spec.

/// Accepts a single value or an array of values.
fn one_or_many<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    struct OneOrMany<T>(PhantomData<T>);
    impl<'de, T: DeserializeOwned> Visitor<'de> for OneOrMany<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a value or an array of values")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(item) = seq.next_element::<T>()? {
                out.push(item);
            }
            Ok(out)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<T>, E> {
            Ok(T::deserialize(de::value::StrDeserializer::<E>::new(v))
                .ok()
                .into_iter()
                .collect())
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Vec<T>, A::Error> {
            Ok(vec![T::deserialize(de::value::MapAccessDeserializer::new(map))?])
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
        fn visit_unit<E: de::Error>(self) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
    }
    deserializer.deserialize_any(OneOrMany(PhantomData))
}

/// An array whose non-array forms are treated as empty.
fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    one_or_many(deserializer)
}

struct Scalar;

enum ScalarValue {
    Str(String),
    Bool(bool),
    Num(f64),
    Other,
}

impl<'de> Visitor<'de> for Scalar {
    type Value = ScalarValue;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any value")
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Str(v.to_string()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Str(v))
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Num(v as f64))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Num(v as f64))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Num(v))
    }
    fn visit_unit<E: de::Error>(self) -> Result<ScalarValue, E> {
        Ok(ScalarValue::Other)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ScalarValue, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(ScalarValue::Other)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ScalarValue, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(ScalarValue::Other)
    }
}

fn lenient_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match deserializer.deserialize_any(Scalar)? {
        ScalarValue::Str(s) => Some(s),
        _ => None,
    })
}

fn lenient_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(match deserializer.deserialize_any(Scalar)? {
        ScalarValue::Bool(b) => b,
        ScalarValue::Num(n) => n != 0.0,
        _ => false,
    })
}

fn lenient_number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    Ok(match deserializer.deserialize_any(Scalar)? {
        ScalarValue::Num(n) => Some(n),
        _ => None,
    })
}

fn lenient_separator<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match deserializer.deserialize_any(Scalar)? {
        ScalarValue::Str(s) => Some(s),
        ScalarValue::Bool(true) => Some("=".to_string()),
        _ => None,
    })
}

// --- Archive -----------------------------------------------------------------

struct Entry {
    offset: usize,
    len: usize,
    raw_len: usize,
}

/// Read access to the embedded spec archive, with parsed specs cached for the
/// lifetime of the process.
pub struct SpecStore {
    entries: HashMap<&'static str, Entry>,
    blobs: &'static [u8],
    cache: Mutex<HashMap<String, Option<Arc<Subcommand>>>>,
    index: OnceLock<Arc<Vec<(String, String)>>>,
}

impl SpecStore {
    pub fn embedded() -> Self {
        Self::from_archive(ARCHIVE)
    }

    fn from_archive(archive: &'static [u8]) -> Self {
        let mut entries = HashMap::new();
        let mut blobs: &'static [u8] = &[];
        if archive.starts_with(MAGIC) && archive.len() >= MAGIC.len() + 4 {
            let mut pos = MAGIC.len();
            let count = read_u32(archive, pos) as usize;
            pos += 4;
            for _ in 0..count {
                if pos + 2 > archive.len() {
                    break;
                }
                let key_len = u16::from_le_bytes([archive[pos], archive[pos + 1]]) as usize;
                pos += 2;
                if pos + key_len + 12 > archive.len() {
                    break;
                }
                let key = std::str::from_utf8(&archive[pos..pos + key_len]).unwrap_or("");
                pos += key_len;
                let entry = Entry {
                    offset: read_u32(archive, pos) as usize,
                    len: read_u32(archive, pos + 4) as usize,
                    raw_len: read_u32(archive, pos + 8) as usize,
                };
                pos += 12;
                entries.insert(key, entry);
            }
            blobs = &archive[pos..];
        }
        SpecStore {
            entries,
            blobs,
            cache: Mutex::new(HashMap::new()),
            index: OnceLock::new(),
        }
    }

    fn raw(&self, key: &str) -> Option<Vec<u8>> {
        let entry = self.entries.get(key)?;
        let data = self.blobs.get(entry.offset..entry.offset + entry.len)?;
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(data).ok()?;
        let mut out = Vec::with_capacity(entry.raw_len);
        std::io::Read::read_to_end(&mut decoder, &mut out).ok()?;
        Some(out)
    }

    pub fn contains(&self, key: &str) -> bool {
        key != "__index__" && !key.starts_with("js/") && self.entries.contains_key(key)
    }

    /// The JavaScript module of a spec, for specs that compute suggestions in code.
    pub fn module(&self, key: &str) -> Option<String> {
        String::from_utf8(self.raw(&format!("js/{key}"))?).ok()
    }

    /// Loads a spec by key (`git`, `aws/s3`). Results, including misses, are cached.
    pub fn load(&self, key: &str) -> Option<Arc<Subcommand>> {
        if let Some(hit) = self.cache.lock().ok()?.get(key) {
            return hit.clone();
        }
        let parsed = if key == "hiruzen" {
            serde_json::from_str::<Subcommand>(OWN_SPEC).ok().map(Arc::new)
        } else if self.contains(key) {
            self.raw(key)
                .and_then(|raw| serde_json::from_slice::<Subcommand>(&raw).ok())
                .map(Arc::new)
        } else {
            None
        };
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key.to_string(), parsed.clone());
        }
        parsed
    }

    /// Top-level command names with their descriptions, sorted by name.
    pub fn index(&self) -> Arc<Vec<(String, String)>> {
        self.index
            .get_or_init(|| {
                let map: HashMap<String, String> = self
                    .raw("__index__")
                    .and_then(|raw| serde_json::from_slice(&raw).ok())
                    .unwrap_or_default();
                let mut list: Vec<(String, String)> = map.into_iter().collect();
                list.sort();
                Arc::new(list)
            })
            .clone()
    }

    #[cfg(test)]
    pub fn keys(&self) -> Vec<&'static str> {
        self.entries.keys().copied().filter(|k| self.contains(k)).collect()
    }
}

fn read_u32(data: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_spec_parses() {
        let store = SpecStore::embedded();
        let keys = store.keys();
        assert!(
            keys.len() > 1000,
            "expected the full spec library, found {}",
            keys.len()
        );
        let failed: Vec<&str> = keys.iter().copied().filter(|key| store.load(key).is_none()).collect();
        assert!(failed.is_empty(), "specs failed to parse: {failed:?}");
    }

    #[test]
    fn hiruzen_has_its_own_spec_covering_every_setting() {
        let spec = SpecStore::embedded().load("hiruzen").expect("hiruzen spec");
        let settings = spec
            .subcommands
            .iter()
            .find(|s| s.name[0] == "settings")
            .expect("settings");
        let offered: Vec<&str> = settings.args[0]
            .suggestions
            .iter()
            .map(|s| s.name[0].as_str())
            .collect();
        let defined: Vec<&str> = crate::settings::DEFINITIONS.iter().map(|d| d.key).collect();
        assert_eq!(offered, defined);
        let theme = spec.subcommands.iter().find(|s| s.name[0] == "theme").expect("theme");
        let themes: Vec<&str> = theme.args[0].suggestions.iter().map(|s| s.name[0].as_str()).collect();
        assert_eq!(themes, crate::settings::THEME_NAMES);
    }

    #[test]
    fn git_spec_has_expected_shape() {
        let store = SpecStore::embedded();
        let git = store.load("git").expect("git spec");
        let checkout = git
            .subcommands
            .iter()
            .find(|s| s.name.iter().any(|n| n == "checkout"))
            .expect("git checkout");
        assert!(!checkout.options.is_empty());
        assert!(!checkout.args.is_empty());
        assert!(store.index().iter().any(|(name, _)| name == "git"));
    }

    #[test]
    fn tolerates_unexpected_field_types() {
        let spec: Subcommand = serde_json::from_str(
            r#"{"name":"x","description":{"oops":1},"options":[{"name":["-a","--all"],"isRepeatable":3,"args":{"name":"n","suggestions":["one",{"name":["two","deux"],"priority":80},5]}}],"args":[{"template":"filepaths","generators":[{"script":["ls"],"splitOn":"\n"}]}]}"#,
        )
        .unwrap();
        assert_eq!(spec.description, None);
        assert!(spec.options[0].is_repeatable);
        let suggestions = &spec.options[0].args[0].suggestions;
        assert_eq!(suggestions.len(), 3);
        assert_eq!(suggestions[1].name, vec!["two", "deux"]);
        assert_eq!(spec.args[0].template, vec!["filepaths"]);
        assert_eq!(spec.args[0].generators[0].script, vec!["ls"]);
    }
}
