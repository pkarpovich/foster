use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::name::SessionName;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub started: i64,
    pub exit_code: Option<i32>,
}

pub fn now() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    i64::try_from(seconds).unwrap_or(0)
}

fn dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/foster/sessions"))
}

fn path_of(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

pub fn save(record: &Record) -> Result<(), String> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let json = serde_json::to_string(record).map_err(|error| format!("encode record: {error}"))?;
    let path = path_of(&dir, &record.name);
    let staged = dir.join(format!(".{}.{}.tmp", record.name, std::process::id()));
    std::fs::write(&staged, json)
        .map_err(|error| format!("write {}: {error}", staged.display()))?;
    std::fs::rename(&staged, &path).map_err(|error| format!("write {}: {error}", path.display()))
}

pub fn load(name: &SessionName) -> Result<Option<Record>, String> {
    let path = path_of(&dir()?, name.as_str());
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let record = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    Ok(Some(record))
}

pub fn load_all() -> Result<Vec<Record>, String> {
    let dir = dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", dir.display())),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("read {}: {error}", dir.display()))?
            .path();
        let is_record = path
            .extension()
            .is_some_and(|extension| extension == "json");
        if !is_record {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        match serde_json::from_str::<Record>(&text) {
            Ok(record) => records.push(record),
            Err(error) => eprintln!("foster: skipping {}: {error}", path.display()),
        }
    }
    records.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(records)
}

pub fn remove(name: &SessionName) -> Result<bool, String> {
    let path = path_of(&dir()?, name.as_str());
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

pub fn mark_exited(name: &SessionName, code: i32) -> Result<(), String> {
    let Some(mut record) = load(name)? else {
        return Ok(());
    };
    record.exit_code = Some(code);
    save(&record)
}
