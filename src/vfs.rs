//! File access used by the library. On desktop it is the real disk. In the
//! browser (wasm) there is no disk, so files live in memory: the web app puts
//! picked files in, the library reads and writes them as usual, and the app
//! takes the results out to download.

use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;
    pub fn read(p: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(p)
    }
    pub fn write(p: &Path, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(p, data)
    }
    pub fn create_dir_all(p: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(p)
    }
    pub fn rename(a: &Path, b: &Path) -> std::io::Result<()> {
        std::fs::rename(a, b)
    }
    pub fn list(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default()
    }
    pub fn remove_dir_all(p: &Path) {
        let _ = std::fs::remove_dir_all(p);
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    thread_local! {
        static FILES: RefCell<BTreeMap<PathBuf, Vec<u8>>> = RefCell::new(BTreeMap::new());
    }
    fn missing(p: &Path) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} not found", p.display()),
        )
    }
    pub fn read(p: &Path) -> std::io::Result<Vec<u8>> {
        FILES.with(|f| f.borrow().get(p).cloned().ok_or_else(|| missing(p)))
    }
    pub fn write(p: &Path, data: &[u8]) -> std::io::Result<()> {
        FILES.with(|f| f.borrow_mut().insert(p.to_path_buf(), data.to_vec()));
        Ok(())
    }
    pub fn create_dir_all(_: &Path) -> std::io::Result<()> {
        Ok(())
    }
    pub fn rename(a: &Path, b: &Path) -> std::io::Result<()> {
        let data = FILES.with(|f| f.borrow_mut().remove(a)).ok_or_else(|| missing(a))?;
        write(b, &data)
    }
    pub fn list(dir: &Path) -> Vec<PathBuf> {
        FILES.with(|f| {
            f.borrow()
                .keys()
                .filter(|k| k.parent() == Some(dir))
                .cloned()
                .collect()
        })
    }
    pub fn remove_dir_all(p: &Path) {
        FILES.with(|f| f.borrow_mut().retain(|k, _| !k.starts_with(p)));
    }
}

pub use imp::{create_dir_all, list, read, remove_dir_all, rename};

pub fn write(p: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    imp::write(p, data.as_ref())
}

pub fn read_to_string(p: &Path) -> std::io::Result<String> {
    let b = read(p)?;
    String::from_utf8(b).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
