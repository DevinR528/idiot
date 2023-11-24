use std::{
    cmp,
    collections::BTreeSet,
    fmt::{self, Write},
    fs,
    io::Read,
    path::Path,
    sync::LazyLock,
};

use anyhow::Context;
use flate2::{bufread::ZlibEncoder, Compression};
use sha1::{Digest, Sha1};

static IGNORE: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
    let mut ignore = BTreeSet::new();

    ignore.insert(".git/".to_string());
    ignore.insert(".idiot/".to_string());

    if let Ok(s) = fs::read_to_string("./.gitignore") {
        for f in s.lines() {
            let pat = f.trim();
            if pat.is_empty() || pat.starts_with('#') {
                continue;
            }
            ignore.insert(pat.to_string());
        }
    }
    ignore
});
fn path_in_ignore(p: &Path) -> bool {
    for s in IGNORE.iter() {
        if p.ends_with(s) {
            return true;
        }
    }
    false
}

const SHA_SIZE: usize = 20;

/// The mode of a git tree object.
#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum Mode {
    FileBlob = 100644,
    ExeBlob = 100755,
    SubDir = 40000,
    SubMod = 160000,
    SymLink = 120000,
}

/// Either blob, tree, or commit.
#[derive(Debug)]
pub enum ObjType {
    Blob {
        /// The file referenced in the tree.
        path: String,
        /// The content the file has.
        content: Vec<u8>,
    },
    Tree {
        /// The folder that this tree represents.
        ///
        /// Only the very top level tree will not have a path.
        path: Option<String>,
        /// Size in bytes of the tree.
        size: usize,
        /// All the objects in the tree.
        objs: Vec<GitObject>,
    },
    #[allow(dead_code)]
    Commit,
}

/// This is an object in a git tree.
#[derive(Debug)]
pub struct GitObject {
    /// The file mode.
    pub mode: Mode,
    /// The type of tree this is.
    pub obj_type: ObjType,
    /// The SHA1 checksum ID of the object in the tree. This is the non hex encoded string.
    ///
    /// A `None` means the file is to be deleted.
    pub sha: Option<Vec<u8>>,
}

impl Mode {
    pub fn new(kind: usize) -> Self {
        match kind {
            100644 => Self::FileBlob,
            100755 => Self::ExeBlob,
            40000 => Self::SubDir,
            160000 => Self::SubMod,
            120000 => Self::SymLink,
            _ => panic!("not a valid mode {}", kind),
        }
    }
}

impl fmt::Display for ObjType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObjType::Blob { .. } => f.write_str("blob"),
            ObjType::Tree { .. } => f.write_str("tree"),
            ObjType::Commit => f.write_str("commit"),
        }
    }
}

impl PartialOrd for GitObject {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for GitObject {
    fn eq(&self, other: &Self) -> bool {
        self.sha == other.sha
    }
}
impl Eq for GitObject {}
impl Ord for GitObject {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        match (&self.obj_type, &other.obj_type) {
            (
                ObjType::Blob { path: a, .. } | ObjType::Tree { path: Some(a), .. },
                ObjType::Blob { path: b, .. } | ObjType::Tree { path: Some(b), .. },
            ) => a.cmp(b),

            (ObjType::Blob { .. }, ObjType::Tree { .. }) => cmp::Ordering::Greater,
            (ObjType::Blob { .. }, ObjType::Commit) => cmp::Ordering::Greater,

            (ObjType::Tree { path: None, .. }, ObjType::Tree { .. }) => cmp::Ordering::Less,
            (ObjType::Tree { .. }, ObjType::Tree { path: None, .. }) => cmp::Ordering::Greater,
            (ObjType::Tree { .. }, ObjType::Blob { .. }) => cmp::Ordering::Less,
            (ObjType::Tree { .. }, ObjType::Commit) => cmp::Ordering::Greater,

            (ObjType::Commit, ObjType::Blob { .. }) => cmp::Ordering::Less,
            (ObjType::Commit, ObjType::Tree { .. }) => cmp::Ordering::Less,
            (ObjType::Commit, ObjType::Commit) => cmp::Ordering::Equal,
        }
    }
}

impl GitObject {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let tree_bytes = bytes.split(|ch| ch == &b'\0').collect::<Vec<&[u8]>>();

        let header = tree_bytes[0]
            .split(|ch| ch == &b' ')
            .collect::<Vec<&[u8]>>();
        if let [b"tree", size_bytes] = header.as_slice() {
            let size = usize_from_bytes(size_bytes).unwrap();
            let mut first = vec![];
            first.extend_from_slice(tree_bytes[1]);
            first.push(b'\0');
            first.extend_from_slice(&tree_bytes[2][..SHA_SIZE]);

            let mut objs = vec![first];
            objs.extend(tree_bytes[2..].windows(2).map(|slice| {
                let mut obj = vec![];
                obj.extend_from_slice(&slice[0][SHA_SIZE..]);
                obj.push(b'\0');
                obj.extend_from_slice(&slice[1][..SHA_SIZE]);
                obj
            }));
            GitObject {
                mode: Mode::SubDir,
                obj_type: ObjType::Tree {
                    // Top level will not have name
                    path: None,
                    size,
                    objs: objs.iter().map(|b| GitObject::from_bytes(b)).collect(),
                },
                sha: None,
            }
        } else {
            let mut split = bytes.splitn(2, |ch| ch == &b' ');
            let mode = match split.next().map(usize_from_bytes) {
                Some(Ok(m)) => Mode::new(m),
                _ => panic!("not a number"),
            };
            let (path, sha) = match split
                .next()
                .map(|b| b.split(|ch| ch == &b'\0').collect::<Vec<&[u8]>>())
                .as_deref()
            {
                Some(&[name, sha]) if !sha.is_empty() => (
                    String::from_utf8(name.to_vec()).expect("name is utf8"),
                    Some(sha.to_vec()),
                ),
                Some(&[name, sha]) if sha.is_empty() => (
                    String::from_utf8(name.to_vec()).expect("name is utf8"),
                    None,
                ),
                Some(_) | None => panic!("invalid tree object, no name or sha"),
            };
            GitObject {
                mode,
                obj_type: ObjType::Blob {
                    path,
                    content: "NOT REAL YET".into(),
                },
                sha,
            }
        }
    }

    pub fn from_path<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();

        if path.is_dir() {
            let mut objs = vec![];
            for e in fs::read_dir(path)? {
                let p = e?.path();
                if path_in_ignore(&p) {
                    continue;
                }
                objs.push(GitObject::from_path(p)?)
            }

            // git will always alphabetically sort objects in the tree
            objs.sort();

            let bytes = objs
                .iter()
                .flat_map(|o| o.tree_content_bytes())
                .collect::<Vec<u8>>();
            let mut content = format!("tree {}\0", bytes.len()).into_bytes();
            content.extend_from_slice(&bytes);

            let (sha, _content) = compress_and_hash(&content)?;

            let path = path
                .components()
                .last()
                .unwrap()
                .as_os_str()
                .to_string_lossy()
                .to_string();
            Ok(Self {
                mode: Mode::SubDir,
                obj_type: ObjType::Tree {
                    path: Some(path),
                    size: bytes.len(),
                    objs,
                },
                sha: Some(sha),
            })
        } else {
            let file_content = fs::read(path).context("read of file content")?;
            let mut content = format!("blob {}\0", file_content.len()).into_bytes();
            content.extend_from_slice(&file_content);
            let (sha, enc_content) = compress_and_hash(&content)?;

            let path = path
                .components()
                .last()
                .unwrap()
                .as_os_str()
                .to_string_lossy()
                .to_string();
            Ok(Self {
                mode: Mode::FileBlob,
                obj_type: ObjType::Blob {
                    path,
                    content: enc_content,
                },
                sha: Some(sha),
            })
        }
    }

    // fn tree_content_size(&self) -> usize {
    //     // [mode] [Object name]\0[SHA-1 in binary format]
    //     let path_len = match &self.obj_type {
    //         ObjType::Blob { path, .. } => path.len(),
    //         ObjType::Tree { path, .. } => path.as_ref().map_or(0, |p| p.len()),
    //         ObjType::Commit => todo!(),
    //     };
    //     // I think the sha.map_or(1, ...) will equate to sha being \0
    //     (self.mode as usize).to_string().len()
    //         + 1
    //         + path_len
    //         + 1
    //         + self.sha.as_ref().map_or(1, |s| s.len())
    // }

    pub fn tree_content_bytes(&self) -> Vec<u8> {
        // [mode] [Object name]\0[SHA-1 in binary format]
        let path = match &self.obj_type {
            ObjType::Blob { path, .. } => path,
            ObjType::Tree {
                path: Some(path), ..
            } => path,
            ObjType::Tree { .. } => todo!(),
            ObjType::Commit => todo!(),
        };
        // I think the sha.map_or("\0", ...) will equate to a deleted object
        let mut bytes = format!("{} {}\0", (self.mode as usize), path).into_bytes();
        bytes.extend_from_slice(self.sha.as_ref().map_or(b"\0", |s| s));
        bytes
    }

    pub fn as_path_str(&self) -> &str {
        match &self.obj_type {
            ObjType::Blob { path, .. } => path,
            ObjType::Tree {
                path: Some(path), ..
            } => path,
            ObjType::Tree { .. } => todo!(),
            ObjType::Commit => todo!(),
        }
    }

    pub fn to_full_string(&self) -> String {
        let mut res = String::new();
        write!(res, "{}", self.mode as usize).expect("valid to write to a string");
        write!(res, " {}", self.obj_type).expect("valid to write to a string");

        if let Some(sha) = &self.sha {
            write!(res, " {}", hex::encode(sha)).expect("valid to write to a string");
        }

        if let ObjType::Blob { path, .. }
        | ObjType::Tree {
            path: Some(path), ..
        } = &self.obj_type
        {
            write!(res, " {}", path).expect("valid to write to a string");
        }

        res
    }
}

fn usize_from_bytes(bytes: &[u8]) -> anyhow::Result<usize> {
    String::from_utf8(bytes.to_vec())?
        .parse()
        .with_context(|| format!("invalid number {}", String::from_utf8_lossy(bytes)))
}

/// Returns the bytes of the SHA1 hash and the compressed `bytes`.
///
/// The hash is taken from the result of compressing `bytes`.
fn compress_and_hash(bytes: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut enc_content = vec![];
    ZlibEncoder::new(bytes, Compression::default()).read_to_end(&mut enc_content)?;

    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    let sha = hasher.finalize().to_vec();
    Ok((sha, enc_content))
}
