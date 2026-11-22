//! The app's file access (DESIGN.md, Ephemeral mode): `std::fs`'s calls, with
//! paths below a mounted root served from an overlay in RAM.
//!
//! An overlay is a tree of folders, files and symlinks held in memory. Mounted
//! over a library on disk (its lower layer), a folder lists the disk's
//! entries the first time it's looked at, and a file read from there is read
//! from the disk; every write, rename, link or removal changes the tree only,
//! so the disk is never written. Copies, renames and hard links of a file
//! still on disk are references to it, holding no RAM. Mounted with no lower
//! layer, the tree starts empty.
//!
//! Every other path goes to `std::fs` as it is. clippy.toml disallows the
//! calls that would go round this module (`std::fs`'s, `Path::exists` and
//! the like, `image::open`).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::SystemTime;

pub use std::fs::{Permissions, TryLockError};

// ------------------------------------------------------------------ mounts

static MOUNTS: RwLock<Vec<Arc<Overlay>>> = RwLock::new(Vec::new());

/// A tree in RAM standing in for the folder `root`.
pub struct Overlay {
    root: PathBuf,
    lower: Option<PathBuf>,
    tree: Mutex<Node>,
    used: Arc<Used>,
}

/// The bytes the overlay's files hold in RAM, and whether to keep them out
/// of swap.
struct Used {
    bytes: AtomicU64,
    pin: bool,
}

/// Mounts an overlay at `root` (absolute, and not inside or around another
/// mount): reads fall through to `lower` where it has the file, writes stay
/// in RAM. `pin`: ask the OS to keep the files' memory out of swap
/// ([`pin_failed`] says whether it refused).
pub fn mount(root: &Path, lower: Option<&Path>, pin: bool) -> io::Result<Arc<Overlay>> {
    let root = normal(root);
    let mut mounts = MOUNTS.write().unwrap_or_else(|e| e.into_inner());
    if mounts.iter().any(|m| m.root.starts_with(&root) || root.starts_with(&m.root)) {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is mounted already", root.display())));
    }
    let lower = lower.map(normal);
    let now = SystemTime::now();
    let tree = Node::Dir(Dir { kids: None, lower: lower.clone(), mtime: now });
    let o = Arc::new(Overlay {
        root,
        lower,
        tree: Mutex::new(tree),
        used: Arc::new(Used { bytes: AtomicU64::new(0), pin }),
    });
    mounts.push(o.clone());
    Ok(o)
}

/// Unmounts the overlay at `root`: its files are dropped from RAM once no
/// open file holds them.
pub fn unmount(root: &Path) {
    let root = normal(root);
    MOUNTS.write().unwrap_or_else(|e| e.into_inner()).retain(|m| m.root != root);
}

/// The overlay `path` is in, if any.
pub fn overlay(path: &Path) -> Option<Arc<Overlay>> {
    let p = normal(path);
    MOUNTS.read().unwrap_or_else(|e| e.into_inner()).iter().find(|m| p.starts_with(&m.root)).cloned()
}

/// `path` is in an overlay (no real file there to open in another app).
pub fn in_ram(path: &Path) -> bool {
    overlay(path).is_some()
}

impl Overlay {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The library on disk read through, if any.
    pub fn lower(&self) -> Option<&Path> {
        self.lower.as_deref()
    }

    /// The bytes held in RAM by the overlay's files.
    pub fn used(&self) -> u64 {
        self.used.bytes.load(Ordering::Relaxed)
    }

    fn lock(&self) -> MutexGuard<'_, Node> {
        self.tree.lock().unwrap_or_else(|e| e.into_inner())
    }
}

static PIN_FAILED: AtomicBool = AtomicBool::new(false);

/// The OS refused to keep some of an overlay's memory out of swap.
pub fn pin_failed() -> bool {
    PIN_FAILED.load(Ordering::Relaxed)
}

/// Asks the OS to keep `buf`'s pages in RAM (best effort).
fn pin(buf: &[u8]) {
    if buf.is_empty() {
        return;
    }
    #[cfg(unix)]
    let ok = unsafe { libc::mlock(buf.as_ptr().cast(), buf.len()) } == 0;
    #[cfg(windows)]
    let ok = unsafe { windows_sys::Win32::System::Memory::VirtualLock(buf.as_ptr() as *const _, buf.len()) } != 0;
    #[cfg(not(any(unix, windows)))]
    let ok = false;
    if !ok {
        PIN_FAILED.store(true, Ordering::Relaxed);
    }
}

/// `p` absolute, without `.` or `..` (lexically: an overlay's symlinks are
/// followed by [`Overlay::resolve`]).
fn normal(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

// ------------------------------------------------------------------ the tree

enum Node {
    Dir(Dir),
    File(FileNode),
    Link(PathBuf),
}

struct Dir {
    /// None: not listed yet from `lower`.
    kids: Option<BTreeMap<OsString, Node>>,
    lower: Option<PathBuf>,
    mtime: SystemTime,
}

#[derive(Clone)]
struct FileNode {
    data: Data,
    mtime: SystemTime,
}

#[derive(Clone)]
enum Data {
    Mem(Arc<Content>),
    /// A file of the lower layer, unchanged.
    Lower {
        path: PathBuf,
        len: u64,
    },
}

/// A file's bytes in RAM, shared by its hard links and open handles.
pub struct Content {
    bytes: RwLock<Vec<u8>>,
    used: Arc<Used>,
}

impl Content {
    fn new(bytes: Vec<u8>, used: &Arc<Used>) -> Arc<Content> {
        used.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if used.pin {
            pin(&bytes);
        }
        Arc::new(Content { bytes: RwLock::new(bytes), used: used.clone() })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Vec<u8>> {
        self.bytes.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Changes the bytes, keeping the count of bytes in RAM.
    fn change<T>(&self, f: impl FnOnce(&mut Vec<u8>) -> T) -> T {
        let mut b = self.bytes.write().unwrap_or_else(|e| e.into_inner());
        let (before, ptr) = (b.len() as u64, b.as_ptr());
        let r = f(&mut b);
        let after = b.len() as u64;
        if after >= before {
            self.used.bytes.fetch_add(after - before, Ordering::Relaxed);
        } else {
            self.used.bytes.fetch_sub(before - after, Ordering::Relaxed);
        }
        if self.used.pin && (b.as_ptr() != ptr || after > before) {
            pin(&b);
        }
        r
    }
}

impl Drop for Content {
    fn drop(&mut self) {
        let n = self.read().len() as u64;
        self.used.bytes.fetch_sub(n, Ordering::Relaxed);
    }
}

fn not_found() -> io::Error {
    io::ErrorKind::NotFound.into()
}

fn err(kind: io::ErrorKind) -> io::Error {
    kind.into()
}

impl Dir {
    fn empty() -> Dir {
        Dir { kids: Some(BTreeMap::new()), lower: None, mtime: SystemTime::now() }
    }

    /// The entries, listed from the lower layer the first time.
    fn kids(&mut self) -> io::Result<&mut BTreeMap<OsString, Node>> {
        if self.kids.is_none() {
            let mut kids = BTreeMap::new();
            if let Some(dir) = &self.lower {
                let rd = match std::fs::read_dir(dir) {
                    Ok(rd) => Some(rd),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e),
                };
                for e in rd.into_iter().flatten() {
                    let e = e?;
                    let path = e.path();
                    let m = std::fs::symlink_metadata(&path)?;
                    let mtime = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let node = if m.file_type().is_symlink() {
                        Node::Link(std::fs::read_link(&path)?)
                    } else if m.is_dir() {
                        Node::Dir(Dir { kids: None, lower: Some(path), mtime })
                    } else {
                        Node::File(FileNode { data: Data::Lower { path, len: m.len() }, mtime })
                    };
                    kids.insert(e.file_name(), node);
                }
            }
            self.kids = Some(kids);
        }
        Ok(self.kids.as_mut().expect("listed"))
    }
}

/// The node at `parts` below `node` (links not followed).
fn walk<'a>(mut node: &'a mut Node, parts: &[OsString]) -> io::Result<&'a mut Node> {
    for p in parts {
        node = match node {
            Node::Dir(d) => d.kids()?.get_mut(p).ok_or_else(not_found)?,
            _ => return Err(err(io::ErrorKind::NotADirectory)),
        };
    }
    Ok(node)
}

/// The folder holding `parts`' last part, and that part.
fn parent<'a, 'p>(tree: &'a mut Node, parts: &'p [OsString]) -> io::Result<(&'a mut Dir, &'p OsStr)> {
    let (name, dir) = parts.split_last().ok_or_else(|| err(io::ErrorKind::InvalidInput))?;
    match walk(tree, dir)? {
        Node::Dir(d) => Ok((d, name)),
        _ => Err(err(io::ErrorKind::NotADirectory)),
    }
}

/// Where a path leads once an overlay's symlinks are followed.
enum Place {
    In(Arc<Overlay>, Vec<OsString>),
    Disk(PathBuf),
}

fn place(path: &Path, follow_last: bool) -> io::Result<Place> {
    let mut p = normal(path);
    for _ in 0..40 {
        let Some(o) = overlay(&p) else { return Ok(Place::Disk(p)) };
        let parts: Vec<OsString> =
            p.strip_prefix(&o.root).unwrap_or(Path::new("")).iter().map(|s| s.to_os_string()).collect();
        match o.resolve(&parts, follow_last)? {
            Ok(parts) => return Ok(Place::In(o, parts)),
            Err(next) => p = next,
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, "too many levels of symlinks"))
}

impl Overlay {
    /// `parts` with no symlink among them (the last too if `follow_last`),
    /// or the path a symlink leads to, to be placed again.
    fn resolve(&self, parts: &[OsString], follow_last: bool) -> io::Result<Result<Vec<OsString>, PathBuf>> {
        let mut tree = self.lock();
        let mut node: &mut Node = &mut tree;
        for (i, p) in parts.iter().enumerate() {
            let Node::Dir(d) = node else { return Ok(Ok(parts.to_vec())) };
            let Some(next) = d.kids()?.get_mut(p) else { return Ok(Ok(parts.to_vec())) };
            let last = i + 1 == parts.len();
            if let Node::Link(target) = next
                && (!last || follow_last)
            {
                let mut to = self.root.clone();
                to.extend(&parts[..i]);
                let to = match (&self.lower, target.is_absolute()) {
                    (Some(lower), true) if target.starts_with(lower) => {
                        self.root.join(target.strip_prefix(lower).unwrap_or(target))
                    }
                    _ => to.join(&*target),
                };
                let mut to = normal(&to);
                to.extend(&parts[i + 1..]);
                return Ok(Err(to));
            }
            node = next;
        }
        Ok(Ok(parts.to_vec()))
    }

    fn path_of(&self, parts: &[OsString]) -> PathBuf {
        let mut p = self.root.clone();
        p.extend(parts);
        p
    }

    fn meta(&self, parts: &[OsString]) -> io::Result<Metadata> {
        let mut tree = self.lock();
        let node = walk(&mut tree, parts)?;
        Ok(match node {
            Node::Dir(d) => Metadata::mem(Kind::Dir, 0, d.mtime),
            Node::Link(t) => Metadata::mem(Kind::Symlink, t.as_os_str().len() as u64, SystemTime::UNIX_EPOCH),
            Node::File(f) => {
                let len = match &f.data {
                    Data::Mem(c) => c.read().len() as u64,
                    Data::Lower { len, .. } => *len,
                };
                Metadata::mem(Kind::File, len, f.mtime)
            }
        })
    }

    fn file(&self, parts: &[OsString]) -> io::Result<FileNode> {
        let mut tree = self.lock();
        match walk(&mut tree, parts)? {
            Node::File(f) => Ok(f.clone()),
            Node::Dir(_) => Err(err(io::ErrorKind::IsADirectory)),
            Node::Link(_) => Err(not_found()),
        }
    }

    /// Puts `node` at `parts` (`replace`: over a file or link there).
    fn insert(&self, parts: &[OsString], node: Node, replace: bool) -> io::Result<()> {
        let mut tree = self.lock();
        let (dir, name) = parent(&mut tree, parts)?;
        let kids = dir.kids()?;
        match kids.get(name) {
            Some(Node::Dir(_)) => return Err(err(io::ErrorKind::AlreadyExists)),
            Some(_) if !replace => return Err(err(io::ErrorKind::AlreadyExists)),
            _ => {}
        }
        kids.insert(name.to_os_string(), node);
        dir.mtime = SystemTime::now();
        Ok(())
    }

    fn take(&self, parts: &[OsString], ok: impl Fn(&mut Node) -> io::Result<()>) -> io::Result<Node> {
        let mut tree = self.lock();
        let (dir, name) = parent(&mut tree, parts)?;
        let kids = dir.kids()?;
        ok(kids.get_mut(name).ok_or_else(not_found)?)?;
        let node = kids.remove(name).expect("there");
        dir.mtime = SystemTime::now();
        Ok(node)
    }

    fn new_file(&self, bytes: Vec<u8>) -> Node {
        Node::File(FileNode { data: Data::Mem(Content::new(bytes, &self.used)), mtime: SystemTime::now() })
    }

    /// The file at `parts` in RAM (copied up from the lower layer), made if
    /// missing and `create`.
    fn content(&self, parts: &[OsString], create: bool, create_new: bool) -> io::Result<Arc<Content>> {
        let mut tree = self.lock();
        let (dir, name) = parent(&mut tree, parts)?;
        let kids = dir.kids()?;
        match kids.get_mut(name) {
            Some(_) if create_new => Err(err(io::ErrorKind::AlreadyExists)),
            Some(Node::File(f)) => match &f.data {
                Data::Mem(c) => Ok(c.clone()),
                Data::Lower { path, .. } => {
                    let c = Content::new(std::fs::read(path)?, &self.used);
                    f.data = Data::Mem(c.clone());
                    Ok(c)
                }
            },
            Some(Node::Dir(_)) => Err(err(io::ErrorKind::IsADirectory)),
            Some(Node::Link(_)) => Err(not_found()),
            None if create || create_new => {
                let c = Content::new(Vec::new(), &self.used);
                kids.insert(
                    name.to_os_string(),
                    Node::File(FileNode { data: Data::Mem(c.clone()), mtime: SystemTime::now() }),
                );
                dir.mtime = SystemTime::now();
                Ok(c)
            }
            None => Err(not_found()),
        }
    }

    fn touch(&self, parts: &[OsString]) {
        if let Ok(Node::File(f)) = walk(&mut self.lock(), parts) {
            f.mtime = SystemTime::now();
        }
    }
}

// ------------------------------------------------------------------ metadata

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType(Kind);

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.0 == Kind::Dir
    }
    pub fn is_file(&self) -> bool {
        self.0 == Kind::File
    }
    pub fn is_symlink(&self) -> bool {
        self.0 == Kind::Symlink
    }
}

impl From<std::fs::FileType> for FileType {
    fn from(t: std::fs::FileType) -> Self {
        FileType(if t.is_symlink() {
            Kind::Symlink
        } else if t.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        })
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    kind: Kind,
    len: u64,
    mtime: SystemTime,
    disk: Option<std::fs::Metadata>,
}

impl Metadata {
    fn mem(kind: Kind, len: u64, mtime: SystemTime) -> Metadata {
        Metadata { kind, len, mtime, disk: None }
    }
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }
    pub fn is_file(&self) -> bool {
        self.kind == Kind::File
    }
    pub fn is_symlink(&self) -> bool {
        self.kind == Kind::Symlink
    }
    pub fn file_type(&self) -> FileType {
        FileType(self.kind)
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(self.mtime)
    }
    pub fn accessed(&self) -> io::Result<SystemTime> {
        self.disk.as_ref().map_or(Ok(self.mtime), |m| m.accessed())
    }
    pub fn created(&self) -> io::Result<SystemTime> {
        self.disk.as_ref().map_or(Ok(self.mtime), |m| m.created())
    }
    /// The file's own metadata, for a file on disk.
    pub fn disk(&self) -> Option<&std::fs::Metadata> {
        self.disk.as_ref()
    }
}

impl From<std::fs::Metadata> for Metadata {
    fn from(m: std::fs::Metadata) -> Self {
        Metadata {
            kind: FileType::from(m.file_type()).0,
            len: m.len(),
            mtime: m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            disk: Some(m),
        }
    }
}

pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    match place(path.as_ref(), true)? {
        Place::Disk(p) => std::fs::metadata(p).map(Into::into),
        Place::In(o, parts) => o.meta(&parts),
    }
}

pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    match place(path.as_ref(), false)? {
        Place::Disk(p) => std::fs::symlink_metadata(p).map(Into::into),
        Place::In(o, parts) => o.meta(&parts),
    }
}

/// The bytes a file holds in RAM: 0 for one still on disk (or not in an
/// overlay).
pub fn ram_held(path: impl AsRef<Path>) -> u64 {
    match place(path.as_ref(), true) {
        Ok(Place::In(o, parts)) => match o.file(&parts).map(|f| f.data) {
            Ok(Data::Mem(c)) => c.read().len() as u64,
            _ => 0,
        },
        _ => 0,
    }
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok()
}

pub fn try_exists(path: impl AsRef<Path>) -> io::Result<bool> {
    match metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn is_dir(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_dir())
}

pub fn is_file(path: impl AsRef<Path>) -> bool {
    metadata(path).is_ok_and(|m| m.is_file())
}

/// `Path`'s file system queries, through the overlay.
pub trait PathExt {
    fn vfs_exists(&self) -> bool;
    fn vfs_try_exists(&self) -> io::Result<bool>;
    fn vfs_is_dir(&self) -> bool;
    fn vfs_is_file(&self) -> bool;
    fn vfs_is_symlink(&self) -> bool;
    fn vfs_metadata(&self) -> io::Result<Metadata>;
    fn vfs_symlink_metadata(&self) -> io::Result<Metadata>;
    fn vfs_read_dir(&self) -> io::Result<ReadDir>;
    fn vfs_read_link(&self) -> io::Result<PathBuf>;
    fn vfs_canonicalize(&self) -> io::Result<PathBuf>;
}

impl PathExt for Path {
    fn vfs_exists(&self) -> bool {
        exists(self)
    }
    fn vfs_try_exists(&self) -> io::Result<bool> {
        try_exists(self)
    }
    fn vfs_is_dir(&self) -> bool {
        is_dir(self)
    }
    fn vfs_is_file(&self) -> bool {
        is_file(self)
    }
    fn vfs_is_symlink(&self) -> bool {
        symlink_metadata(self).is_ok_and(|m| m.is_symlink())
    }
    fn vfs_metadata(&self) -> io::Result<Metadata> {
        metadata(self)
    }
    fn vfs_symlink_metadata(&self) -> io::Result<Metadata> {
        symlink_metadata(self)
    }
    fn vfs_read_dir(&self) -> io::Result<ReadDir> {
        read_dir(self)
    }
    fn vfs_read_link(&self) -> io::Result<PathBuf> {
        read_link(self)
    }
    fn vfs_canonicalize(&self) -> io::Result<PathBuf> {
        canonicalize(self)
    }
}

// ------------------------------------------------------------------ whole files

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    match place(path.as_ref(), true)? {
        Place::Disk(p) => std::fs::read(p),
        Place::In(o, parts) => match o.file(&parts)?.data {
            Data::Mem(c) => Ok(c.read().clone()),
            Data::Lower { path, .. } => std::fs::read(path),
        },
    }
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> io::Result<()> {
    match place(path.as_ref(), true)? {
        Place::Disk(p) => std::fs::write(p, data),
        Place::In(o, parts) => {
            let c = o.content(&parts, true, false)?;
            c.change(|b| {
                b.clear();
                b.extend_from_slice(data.as_ref());
            });
            o.touch(&parts);
            Ok(())
        }
    }
}

/// Copies a file; within an overlay, a file still on disk stays a
/// reference to it.
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let (from, to) = (place(from.as_ref(), true)?, place(to.as_ref(), true)?);
    match (from, to) {
        (Place::Disk(a), Place::Disk(b)) => std::fs::copy(a, b),
        (from, Place::In(o, parts)) => {
            let (node, len) = match from {
                Place::In(fo, fparts) => {
                    let f = fo.file(&fparts)?;
                    match f.data {
                        Data::Lower { path, len } => {
                            (Node::File(FileNode { data: Data::Lower { path, len }, mtime: SystemTime::now() }), len)
                        }
                        Data::Mem(c) => {
                            let b = c.read().clone();
                            let n = b.len() as u64;
                            (o.new_file(b), n)
                        }
                    }
                }
                Place::Disk(p) => {
                    let b = std::fs::read(p)?;
                    let n = b.len() as u64;
                    (o.new_file(b), n)
                }
            };
            o.insert(&parts, node, true)?;
            Ok(len)
        }
        (Place::In(fo, fparts), Place::Disk(b)) => {
            let bytes = read(fo.path_of(&fparts))?;
            std::fs::write(b, &bytes)?;
            Ok(bytes.len() as u64)
        }
    }
}

// ------------------------------------------------------------------ folders

pub fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    match place(path.as_ref(), false)? {
        Place::Disk(p) => std::fs::create_dir(p),
        Place::In(_, parts) if parts.is_empty() => Err(err(io::ErrorKind::AlreadyExists)),
        Place::In(o, parts) => o.insert(&parts, Node::Dir(Dir::empty()), false),
    }
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if overlay(path).is_none() {
        return std::fs::create_dir_all(path);
    }
    if is_dir(path) {
        return Ok(());
    }
    if let Some(up) = normal(path).parent() {
        create_dir_all(up)?;
    }
    match create_dir(path) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && is_dir(path) => Ok(()),
        r => r,
    }
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    match place(path.as_ref(), false)? {
        Place::Disk(p) => std::fs::remove_file(p),
        Place::In(o, parts) => o
            .take(&parts, |n| match n {
                Node::Dir(_) => Err(err(io::ErrorKind::IsADirectory)),
                _ => Ok(()),
            })
            .map(drop),
    }
}

pub fn remove_dir(path: impl AsRef<Path>) -> io::Result<()> {
    match place(path.as_ref(), false)? {
        Place::Disk(p) => std::fs::remove_dir(p),
        Place::In(o, parts) => o
            .take(&parts, |n| match n {
                Node::Dir(d) if d.kids.as_ref().is_some_and(|k| k.is_empty()) => Ok(()),
                Node::Dir(d) => match d.kids()?.is_empty() {
                    true => Ok(()),
                    false => Err(err(io::ErrorKind::DirectoryNotEmpty)),
                },
                _ => Err(err(io::ErrorKind::NotADirectory)),
            })
            .map(drop),
    }
}

pub fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    match place(path.as_ref(), false)? {
        Place::Disk(p) => std::fs::remove_dir_all(p),
        Place::In(o, parts) => o
            .take(&parts, |n| match n {
                Node::Dir(_) => Ok(()),
                _ => Err(err(io::ErrorKind::NotADirectory)),
            })
            .map(drop),
    }
}

pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    match (place(from.as_ref(), false)?, place(to.as_ref(), false)?) {
        (Place::Disk(a), Place::Disk(b)) => std::fs::rename(a, b),
        (Place::In(a, from), Place::In(b, to)) if Arc::ptr_eq(&a, &b) => {
            if from == to {
                return Ok(());
            }
            if to.starts_with(&from) {
                return Err(err(io::ErrorKind::InvalidInput));
            }
            let is_dir = matches!(a.meta(&from)?.kind, Kind::Dir);
            match a.meta(&to) {
                Ok(m) if m.kind == Kind::Dir && !is_dir => return Err(err(io::ErrorKind::IsADirectory)),
                Ok(m) if m.kind != Kind::Dir && is_dir => return Err(err(io::ErrorKind::NotADirectory)),
                Ok(m) if m.kind == Kind::Dir => remove_dir(a.path_of(&to))?,
                _ => {}
            }
            // the parent of `to` must be there before `from` is taken out
            if !matches!(a.meta(&to[..to.len().saturating_sub(1)])?.kind, Kind::Dir) {
                return Err(err(io::ErrorKind::NotADirectory));
            }
            let node = a.take(&from, |_| Ok(()))?;
            a.insert(&to, node, true)
        }
        _ => Err(err(io::ErrorKind::CrossesDevices)),
    }
}

pub fn hard_link(original: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    match (place(original.as_ref(), true)?, place(link.as_ref(), false)?) {
        (Place::Disk(a), Place::Disk(b)) => std::fs::hard_link(a, b),
        (Place::In(a, from), Place::In(b, to)) if Arc::ptr_eq(&a, &b) => {
            let f = a.file(&from)?;
            a.insert(&to, Node::File(f), false)
        }
        _ => Err(err(io::ErrorKind::CrossesDevices)),
    }
}

/// A symlink to a file at `link` (`std::os::windows::fs::symlink_file` on
/// Windows).
pub fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    match place(link.as_ref(), false)? {
        Place::Disk(p) => {
            #[cfg(unix)]
            return std::os::unix::fs::symlink(target, p);
            #[cfg(windows)]
            return std::os::windows::fs::symlink_file(target, p);
        }
        Place::In(o, parts) => o.insert(&parts, Node::Link(target.as_ref().to_path_buf()), false),
    }
}

pub fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    match place(path.as_ref(), false)? {
        Place::Disk(p) => std::fs::read_link(p),
        Place::In(o, parts) => match walk(&mut o.lock(), &parts)? {
            Node::Link(t) => Ok(t.clone()),
            _ => Err(err(io::ErrorKind::InvalidInput)),
        },
    }
}

pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    match place(path.as_ref(), true)? {
        Place::Disk(p) => std::fs::canonicalize(p),
        Place::In(o, parts) => {
            o.meta(&parts)?;
            Ok(o.path_of(&parts))
        }
    }
}

/// Sets a file's permissions on disk; in RAM, nothing to set.
pub fn set_permissions(path: impl AsRef<Path>, perm: Permissions) -> io::Result<()> {
    match place(path.as_ref(), true)? {
        Place::Disk(p) => std::fs::set_permissions(p, perm),
        Place::In(o, parts) => o.meta(&parts).map(drop),
    }
}

pub struct DirEntry {
    path: PathBuf,
    name: OsString,
    meta: Option<Metadata>,
    disk: Option<std::fs::DirEntry>,
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }
    /// The entry's type, its symlinks not followed.
    pub fn file_type(&self) -> io::Result<FileType> {
        match (&self.meta, &self.disk) {
            (Some(m), _) => Ok(m.file_type()),
            (_, Some(d)) => d.file_type().map(Into::into),
            _ => Err(not_found()),
        }
    }
    /// The entry's metadata, its symlinks not followed.
    pub fn metadata(&self) -> io::Result<Metadata> {
        match (&self.meta, &self.disk) {
            (Some(m), _) => Ok(m.clone()),
            (_, Some(d)) => d.metadata().map(Into::into),
            _ => Err(not_found()),
        }
    }
}

pub struct ReadDir(Box<dyn Iterator<Item = io::Result<DirEntry>> + Send>);

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

pub fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let given = path.as_ref().to_path_buf();
    match place(&given, true)? {
        Place::Disk(p) => {
            let rd = std::fs::read_dir(p)?;
            Ok(ReadDir(Box::new(
                rd.map(|e| e.map(|d| DirEntry { path: d.path(), name: d.file_name(), meta: None, disk: Some(d) })),
            )))
        }
        Place::In(o, parts) => {
            let names: Vec<OsString> = match walk(&mut o.lock(), &parts)? {
                Node::Dir(d) => d.kids()?.keys().cloned().collect(),
                _ => return Err(err(io::ErrorKind::NotADirectory)),
            };
            let mut out = Vec::with_capacity(names.len());
            for name in names {
                let mut p = parts.clone();
                p.push(name.clone());
                if let Ok(meta) = o.meta(&p) {
                    out.push(Ok(DirEntry { path: given.join(&name), name, meta: Some(meta), disk: None }));
                }
            }
            Ok(ReadDir(Box::new(out.into_iter())))
        }
    }
}

// ------------------------------------------------------------------ open files

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read(&mut self, on: bool) -> &mut Self {
        self.read = on;
        self
    }
    pub fn write(&mut self, on: bool) -> &mut Self {
        self.write = on;
        self
    }
    pub fn append(&mut self, on: bool) -> &mut Self {
        self.append = on;
        self
    }
    pub fn truncate(&mut self, on: bool) -> &mut Self {
        self.truncate = on;
        self
    }
    pub fn create(&mut self, on: bool) -> &mut Self {
        self.create = on;
        self
    }
    pub fn create_new(&mut self, on: bool) -> &mut Self {
        self.create_new = on;
        self
    }

    pub fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        match place(path.as_ref(), true)? {
            Place::Disk(p) => std::fs::OpenOptions::new()
                .read(self.read)
                .write(self.write)
                .append(self.append)
                .truncate(self.truncate)
                .create(self.create)
                .create_new(self.create_new)
                .open(p)
                .map(|f| File(Inner::Disk(f))),
            Place::In(_, _) if self.create_new && symlink_metadata(path.as_ref()).is_ok() => {
                Err(err(io::ErrorKind::AlreadyExists))
            }
            Place::In(o, parts) => {
                let writes = self.write || self.append;
                if !writes && let Data::Lower { path, .. } = o.file(&parts)?.data {
                    return std::fs::File::open(path).map(|f| File(Inner::Disk(f)));
                }
                let content = o.content(&parts, writes && self.create, writes && self.create_new)?;
                if self.truncate && writes {
                    content.change(Vec::clear);
                }
                Ok(File(Inner::Mem(MemFile { content, pos: 0, append: self.append, write: writes, o, parts })))
            }
        }
    }
}

/// An open file: on disk, or in an overlay.
pub struct File(Inner);

enum Inner {
    Disk(std::fs::File),
    Mem(MemFile),
}

struct MemFile {
    content: Arc<Content>,
    pos: u64,
    append: bool,
    write: bool,
    o: Arc<Overlay>,
    parts: Vec<OsString>,
}

impl std::fmt::Debug for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Inner::Disk(d) => d.fmt(f),
            Inner::Mem(m) => write!(f, "File(in RAM: {})", m.o.path_of(&m.parts).display()),
        }
    }
}

impl File {
    pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().read(true).open(path)
    }

    pub fn create(path: impl AsRef<Path>) -> io::Result<File> {
        OpenOptions::new().write(true).create(true).truncate(true).open(path)
    }

    /// The file on disk, if it is one.
    pub fn disk(&self) -> Option<&std::fs::File> {
        match &self.0 {
            Inner::Disk(f) => Some(f),
            Inner::Mem(_) => None,
        }
    }

    pub fn try_clone(&self) -> io::Result<File> {
        Ok(File(match &self.0 {
            Inner::Disk(f) => Inner::Disk(f.try_clone()?),
            Inner::Mem(m) => Inner::Mem(MemFile {
                content: m.content.clone(),
                pos: m.pos,
                append: m.append,
                write: m.write,
                o: m.o.clone(),
                parts: m.parts.clone(),
            }),
        }))
    }

    pub fn set_modified(&self, t: SystemTime) -> io::Result<()> {
        match &self.0 {
            Inner::Disk(f) => f.set_modified(t),
            Inner::Mem(m) => {
                if let Ok(Node::File(f)) = walk(&mut m.o.lock(), &m.parts) {
                    f.mtime = t;
                }
                Ok(())
            }
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        match &self.0 {
            Inner::Disk(f) => f.sync_all(),
            Inner::Mem(_) => Ok(()),
        }
    }

    pub fn set_len(&self, len: u64) -> io::Result<()> {
        match &self.0 {
            Inner::Disk(f) => f.set_len(len),
            Inner::Mem(m) => {
                m.content.change(|b| b.resize(len as usize, 0));
                Ok(())
            }
        }
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        match &self.0 {
            Inner::Disk(f) => f.metadata().map(Into::into),
            Inner::Mem(m) => Ok(Metadata::mem(Kind::File, m.content.read().len() as u64, SystemTime::now())),
        }
    }

    /// Locks the file (on disk; in RAM, only this process sees it anyway).
    pub fn try_lock(&self) -> Result<(), TryLockError> {
        match &self.0 {
            Inner::Disk(f) => f.try_lock(),
            Inner::Mem(_) => Ok(()),
        }
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        match &self.0 {
            Inner::Disk(f) => f.try_lock_shared(),
            Inner::Mem(_) => Ok(()),
        }
    }

    /// Reads `buf.len()` bytes at `at`.
    pub fn read_exact_at(&self, buf: &mut [u8], at: u64) -> io::Result<()> {
        match &self.0 {
            Inner::Disk(f) => crate::fsutil::read_exact_at_disk(f, buf, at),
            Inner::Mem(m) => {
                let b = m.content.read();
                let at = at as usize;
                let src = b.get(at..at + buf.len()).ok_or_else(|| err(io::ErrorKind::UnexpectedEof))?;
                buf.copy_from_slice(src);
                Ok(())
            }
        }
    }

    /// Writes all of `buf` at `at`.
    pub fn write_all_at(&self, buf: &[u8], at: u64) -> io::Result<()> {
        match &self.0 {
            Inner::Disk(f) => crate::fsutil::write_all_at_disk(f, buf, at),
            Inner::Mem(m) => {
                if !m.write {
                    return Err(err(io::ErrorKind::PermissionDenied));
                }
                m.content.change(|b| write_at(b, buf, at as usize));
                m.o.touch(&m.parts);
                Ok(())
            }
        }
    }
}

fn write_at(b: &mut Vec<u8>, buf: &[u8], at: usize) {
    if b.len() < at + buf.len() {
        b.resize(at + buf.len(), 0);
    }
    b[at..at + buf.len()].copy_from_slice(buf);
}

impl Read for File {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        match &mut self.0 {
            Inner::Disk(f) => f.read(out),
            Inner::Mem(m) => {
                let b = m.content.read();
                let at = (m.pos as usize).min(b.len());
                let n = out.len().min(b.len() - at);
                out[..n].copy_from_slice(&b[at..at + n]);
                drop(b);
                m.pos += n as u64;
                Ok(n)
            }
        }
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.0 {
            Inner::Disk(f) => f.write(buf),
            Inner::Mem(m) => {
                if !m.write {
                    return Err(err(io::ErrorKind::PermissionDenied));
                }
                let pos = m.pos as usize;
                let append = m.append;
                let end = m.content.change(|b| {
                    let at = if append { b.len() } else { pos };
                    write_at(b, buf, at);
                    at + buf.len()
                });
                m.pos = end as u64;
                m.o.touch(&m.parts);
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.0 {
            Inner::Disk(f) => f.flush(),
            Inner::Mem(_) => Ok(()),
        }
    }
}

impl Seek for File {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match &mut self.0 {
            Inner::Disk(f) => f.seek(to),
            Inner::Mem(m) => {
                let len = m.content.read().len() as i64;
                let at = match to {
                    SeekFrom::Start(n) => n as i64,
                    SeekFrom::End(d) => len + d,
                    SeekFrom::Current(d) => m.pos as i64 + d,
                };
                if at < 0 {
                    return Err(err(io::ErrorKind::InvalidInput));
                }
                m.pos = at as u64;
                Ok(m.pos)
            }
        }
    }
}

/// Decodes an image file (`image::open`, through the overlay), its format
/// from its first bytes.
pub fn open_image(path: impl AsRef<Path>) -> image::ImageResult<image::DynamicImage> {
    let path = path.as_ref();
    if overlay(path).is_none() {
        #[allow(clippy::disallowed_methods)]
        return image::open(path);
    }
    let bytes = read(path).map_err(image::ImageError::IoError)?;
    image::ImageReader::new(io::Cursor::new(bytes)).with_guessed_format().map_err(image::ImageError::IoError)?.decode()
}

/// An image file's size in pixels, from its header (`image::image_dimensions`,
/// through the overlay).
pub fn image_dimensions(path: impl AsRef<Path>) -> image::ImageResult<(u32, u32)> {
    let path = path.as_ref();
    if overlay(path).is_none() {
        #[allow(clippy::disallowed_methods)]
        return image::image_dimensions(path);
    }
    let bytes = read(path).map_err(image::ImageError::IoError)?;
    image::ImageReader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?
        .into_dimensions()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("increart-vfs-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_new_overlay_holds_everything_in_ram() {
        let r = root("new");
        let o = mount(&r, None, false).unwrap();
        create_dir_all(r.join("a/b")).unwrap();
        write(r.join("a/b/f.txt"), b"hello").unwrap();
        assert_eq!(read_to_string(r.join("a/./b/../b/f.txt")).unwrap(), "hello");
        assert_eq!(o.used(), 5);
        assert!(!std::fs::exists(&r).unwrap(), "nothing on disk");
        // symlinks and hard links
        symlink("b/f.txt", r.join("a/link.txt")).unwrap();
        assert_eq!(read(r.join("a/link.txt")).unwrap(), b"hello");
        assert!(symlink_metadata(r.join("a/link.txt")).unwrap().is_symlink());
        assert!(metadata(r.join("a/link.txt")).unwrap().is_file());
        hard_link(r.join("a/b/f.txt"), r.join("a/hard.txt")).unwrap();
        let mut f = OpenOptions::new().write(true).open(r.join("a/hard.txt")).unwrap();
        f.write_all(b"HE").unwrap();
        assert_eq!(read(r.join("a/b/f.txt")).unwrap(), b"HEllo", "hard links share their bytes");
        drop(f);
        assert_eq!(o.used(), 5);
        // rename, list, remove
        rename(r.join("a/b"), r.join("c")).unwrap();
        let names: Vec<_> = read_dir(r.join("a")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["hard.txt", "link.txt"]);
        assert!(read(r.join("a/link.txt")).is_err(), "its target moved");
        remove_file(r.join("a/hard.txt")).unwrap();
        assert_eq!(o.used(), 5, "c/f.txt still holds them");
        remove_dir_all(r.join("c")).unwrap();
        assert_eq!(o.used(), 0);
        assert_eq!(remove_dir(r.join("a")).unwrap_err().kind(), io::ErrorKind::DirectoryNotEmpty);
        assert_eq!(
            OpenOptions::new().write(true).create_new(true).open(r.join("a/link.txt")).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        unmount(&r);
    }

    /// An image only in RAM: its size and pixels read from there (a ref
    /// image's "use size" had gone, its size unread, in an ephemeral session).
    #[test]
    fn an_image_in_the_overlay_is_read_from_ram() {
        let r = root("image");
        let _o = mount(&r, None, false).unwrap();
        let mut png = Vec::new();
        image::RgbImage::new(96, 64).write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        write(r.join("ref.png"), &png).unwrap();
        assert_eq!(image_dimensions(r.join("ref.png")).unwrap(), (96, 64));
        assert_eq!(open_image(r.join("ref.png")).unwrap().width(), 96);
        unmount(&r);
    }

    #[test]
    fn an_overlay_reads_the_disk_and_never_writes_it() {
        let disk = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(disk.path().join("d")).unwrap();
        std::fs::write(disk.path().join("d/x"), b"disk").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("d/x", disk.path().join("lx")).unwrap();
        let r = root("lower");
        let o = mount(&r, Some(disk.path()), false).unwrap();
        assert_eq!(read(r.join("d/x")).unwrap(), b"disk");
        #[cfg(unix)]
        assert_eq!(read(r.join("lx")).unwrap(), b"disk");
        copy(r.join("d/x"), r.join("y")).unwrap();
        rename(r.join("d"), r.join("e")).unwrap();
        assert_eq!(o.used(), 0, "copies and renames of disk files hold no RAM");
        assert_eq!(read(r.join("e/x")).unwrap(), b"disk");
        let mut f = OpenOptions::new().append(true).open(r.join("y")).unwrap();
        f.write_all(b"!").unwrap();
        assert_eq!(read(r.join("y")).unwrap(), b"disk!");
        remove_file(r.join("e/x")).unwrap();
        write(r.join("e/new"), b"n").unwrap();
        // the disk as it was
        assert_eq!(std::fs::read(disk.path().join("d/x")).unwrap(), b"disk");
        assert!(!disk.path().join("y").exists() && !disk.path().join("e").exists());
        // a file copied out goes to disk
        let out = disk.path().join("exported");
        copy(r.join("y"), &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"disk!");
        unmount(&r);
        assert!(read(r.join("y")).is_err());
    }
}
