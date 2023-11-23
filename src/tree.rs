use std::fmt::Write;

use anyhow::Context;

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
#[derive(Clone, Copy, Debug)]
pub enum Type {
    Blob,
    Tree,
    Commit,
}

/// This is an object in a git tree.
#[derive(Debug)]
pub struct TreeObj {
    /// The file referenced in the tree.
    pub path: String,
    /// The file mode.
    pub mode: Mode,
    /// The type of tree this is.
    pub tree_type: Type,
    /// The SHA1 checksum ID of the object in the tree.
    ///
    /// A `None` means the file is to be deleted.
    pub sha: Option<String>,
    /// The content you want this file to have.
    pub content: String,
}

/// A tree of git objects.
#[derive(Debug)]
pub struct Tree {
    pub size: usize,
    pub objs: Vec<TreeObj>,
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

impl TreeObj {
    pub fn new(bytes: Vec<u8>) -> Self {
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
                Some(hex::encode(sha)),
            ),
            Some(&[name, sha]) if sha.is_empty() => (
                String::from_utf8(name.to_vec()).expect("name is utf8"),
                None,
            ),
            Some(_) | None => panic!("invalid tree object, no name or sha"),
        };
        TreeObj {
            path,
            mode,
            tree_type: Type::Blob,
            sha,
            content: "fds".to_string(),
        }
    }

    pub fn to_full_string(&self) -> String {
        let mut res = String::new();
        write!(res, "{} {}", self.mode as usize, self.path).expect("valid to write to a string");
        if let Some(sha) = &self.sha {
            write!(res, " {}", sha).expect("valid to write to a string");
        }

        res
    }
}

impl Tree {
    pub fn new(bytes: &[u8]) -> Self {
        let tree_bytes = bytes.split(|ch| ch == &b'\0').collect::<Vec<&[u8]>>();

        let header = tree_bytes[0]
            .split(|ch| ch == &b' ')
            .collect::<Vec<&[u8]>>();
        let size = if let [b"tree", size_bytes] = header.as_slice() {
            usize_from_bytes(size_bytes).unwrap()
        } else {
            panic!("not correct tree header format")
        };

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
        Tree {
            size,
            objs: objs.into_iter().map(TreeObj::new).collect(),
        }
    }
}

fn usize_from_bytes(bytes: &[u8]) -> anyhow::Result<usize> {
    String::from_utf8_lossy(bytes)
        .parse()
        .with_context(|| format!("invalid number {}", String::from_utf8_lossy(bytes)))
}
