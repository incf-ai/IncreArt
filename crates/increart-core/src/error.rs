use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    /// A file that exists but can't be parsed (RSN, JSON, PNG structure).
    #[error("{}: {msg}", path.display())]
    Format { path: PathBuf, msg: String },
    #[error("{}: {source}", path.display())]
    Image { path: PathBuf, source: image::ImageError },
    #[error("library {} is already open in another instance", .0.display())]
    Locked(PathBuf),
    #[error("{} is not an IncreArt library (no library.rsn)", .0.display())]
    NotALibrary(PathBuf),
    #[error("library format {0} is not supported by this version")]
    UnsupportedFormat(u32),
    #[error("invalid id {0:?}")]
    InvalidId(String),
    #[error("invalid name {name:?}: {reason}")]
    InvalidName { name: String, reason: &'static str },
    #[error("{0} not found")]
    NotFound(String),
    /// An operation that is well formed but not allowed (e.g. moving a folder
    /// into itself, nesting a project in a project).
    #[error("{0}")]
    Refused(String),
    /// A path without the part an operation needs: a file name, a parent,
    /// or the folder it should be under.
    #[error("{}: {what}", path.display())]
    Path { path: PathBuf, what: &'static str },
    /// Something that couldn't be written out: pixels as PNG or WebP, or a
    /// value as RSN or JSON.
    #[error("{what}: couldn't encode: {msg}")]
    Encode { what: String, msg: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Attaches a path to `io::Error`s.
pub trait IoContext<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|source| Error::Io { path: path.as_ref().to_path_buf(), source })
    }
}

impl Error {
    pub fn format(path: impl AsRef<Path>, msg: impl ToString) -> Self {
        Error::Format { path: path.as_ref().to_path_buf(), msg: msg.to_string() }
    }

    pub fn image(path: impl AsRef<Path>, source: image::ImageError) -> Self {
        Error::Image { path: path.as_ref().to_path_buf(), source }
    }

    pub fn encode(what: impl ToString, msg: impl ToString) -> Self {
        Error::Encode { what: what.to_string(), msg: msg.to_string() }
    }
}

/// A path's parts, as errors naming the path when it hasn't them.
pub trait PathParts {
    /// The last part (none: the path is empty, a root, or ends in `..`).
    fn name(&self) -> Result<&OsStr>;
    /// The path without its last part.
    fn parent_dir(&self) -> Result<&Path>;
    /// The path from `base`, which it must start with.
    fn relative_to(&self, base: &Path) -> Result<&Path>;
}

impl PathParts for Path {
    fn name(&self) -> Result<&OsStr> {
        self.file_name().ok_or_else(|| Error::Path { path: self.to_path_buf(), what: "has no file name" })
    }

    fn parent_dir(&self) -> Result<&Path> {
        self.parent().ok_or_else(|| Error::Path { path: self.to_path_buf(), what: "has no parent folder" })
    }

    fn relative_to(&self, base: &Path) -> Result<&Path> {
        self.strip_prefix(base)
            .map_err(|_| Error::Path { path: self.to_path_buf(), what: "is outside the folder walked" })
    }
}
