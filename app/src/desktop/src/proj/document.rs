//! Comment-preserving edits of a `.proj` file and `schema_version` migrations.

use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, Table, Value};

use super::{CURRENT_SCHEMA_VERSION, ProjError, ProjFile, write_atomic};

/// A `.proj` file opened for editing. Everything not touched by an edit (comments, ordering,
/// whitespace, unknown formatting) is written back byte for byte.
#[derive(Debug, Clone)]
pub struct ProjDocument {
    doc: DocumentMut,
}

/// One migration step: rewrites a document from version `n` to `n + 1`.
type Migration = fn(&mut DocumentMut) -> Result<(), ProjError>;

/// `MIGRATIONS[i]` upgrades schema version `i + 1` to `i + 2`. Append a step when bumping
/// [`CURRENT_SCHEMA_VERSION`]; never edit an existing one.
const MIGRATIONS: &[Migration] = &[];

const _: () = assert!(
    MIGRATIONS.len() + 1 == CURRENT_SCHEMA_VERSION as usize,
    "add a migration when bumping CURRENT_SCHEMA_VERSION"
);

impl ProjDocument {
    pub fn parse(text: &str) -> Result<Self, ProjError> {
        let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
            ProjError::parse_at(text, e.message().to_string(), e.span())
        })?;
        Ok(ProjDocument { doc })
    }

    pub fn load(path: &Path) -> Result<Self, ProjError> {
        let text = std::fs::read_to_string(path).map_err(|source| ProjError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Validate the edited document, then write it atomically. Nothing is written if the result
    /// would not load.
    pub fn save(&self, path: &Path) -> Result<ProjFile, ProjError> {
        let mut upgraded = self.clone();
        migrate(&mut upgraded)?;
        let text = upgraded.to_string();
        let proj = ProjFile::parse(&text)?;
        write_atomic(path, &text)?;
        Ok(proj)
    }

    /// The validated typed view of the current text.
    pub fn to_proj(&self) -> Result<ProjFile, ProjError> {
        ProjFile::parse(&self.to_string())
    }

    pub fn schema_version(&self) -> Result<u32, ProjError> {
        let missing = || ProjError::Parse {
            message: "missing top-level schema_version (expected `schema_version = 1`)".into(),
            line: None,
            column: None,
        };
        let item = self.doc.get("schema_version").ok_or_else(missing)?;
        let n = item.as_integer().ok_or_else(|| ProjError::Parse {
            message: "schema_version must be a whole number".into(),
            line: None,
            column: None,
        })?;
        u32::try_from(n)
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| ProjError::Parse {
                message: format!("schema_version {n} is not valid (must be 1 or higher)"),
                line: None,
                column: None,
            })
    }

    /// Set `project.version` after checking it is a semantic version.
    pub fn set_version(&mut self, version: &str) -> Result<(), ProjError> {
        semver::Version::parse(version).map_err(|e| {
            ProjError::Invalid(vec![super::Issue {
                key: "project.version".into(),
                message: format!("\"{version}\" is not a semantic version: {e}"),
            }])
        })?;
        self.set(&["project", "version"], Value::from(version));
        Ok(())
    }

    pub fn set_string(&mut self, path: &[&str], value: &str) {
        self.set(path, Value::from(value));
    }

    pub fn set_string_array<S: AsRef<str>>(&mut self, path: &[&str], values: &[S]) {
        let array: Array = values.iter().map(|s| s.as_ref()).collect();
        self.set(path, Value::Array(array));
    }

    /// Remove a key. Returns whether it existed.
    pub fn remove(&mut self, path: &[&str]) -> bool {
        let Some((last, parents)) = path.split_last() else {
            return false;
        };
        let mut table: &mut dyn toml_edit::TableLike = self.doc.as_table_mut();
        for key in parents {
            match table.get_mut(key).and_then(Item::as_table_like_mut) {
                Some(t) => table = t,
                None => return false,
            }
        }
        table.remove(last).is_some()
    }

    /// Set a value at a dotted path, creating tables as needed. An existing value keeps its
    /// surrounding comments and spacing.
    pub fn set(&mut self, path: &[&str], mut value: Value) {
        let Some((last, parents)) = path.split_last() else {
            return;
        };
        let mut table: &mut dyn toml_edit::TableLike = self.doc.as_table_mut();
        for key in parents {
            let item = table.entry(key).or_insert_with(|| {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            });
            if item.as_table_like().is_none() {
                *item = Item::Table(Table::new());
            }
            table = item.as_table_like_mut().expect("just ensured a table");
        }
        match table.get_mut(last).and_then(Item::as_value_mut) {
            Some(old) => {
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            None => {
                table.insert(last, Item::Value(value));
            }
        }
    }

    pub(crate) fn inner_mut(&mut self) -> &mut DocumentMut {
        &mut self.doc
    }
}

impl std::fmt::Display for ProjDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.doc)
    }
}

/// Upgrade an older document to [`CURRENT_SCHEMA_VERSION`] in place. Returns whether anything
/// changed. A newer version than this build knows is an error, never a silent downgrade.
pub fn migrate(doc: &mut ProjDocument) -> Result<bool, ProjError> {
    let found = doc.schema_version()?;
    if found > CURRENT_SCHEMA_VERSION {
        return Err(ProjError::UnsupportedSchema {
            found,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if found == CURRENT_SCHEMA_VERSION {
        return Ok(false);
    }
    for step in &MIGRATIONS[(found - 1) as usize..] {
        step(doc.inner_mut())?;
    }
    doc.set(
        &["schema_version"],
        Value::from(i64::from(CURRENT_SCHEMA_VERSION)),
    );
    Ok(true)
}
