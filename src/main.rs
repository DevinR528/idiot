use std::{
    fs,
    io::{self, Read},
};

use anyhow::Context;
use clap::{Parser, Subcommand};
use flate2::{
    bufread::{ZlibDecoder, ZlibEncoder},
    Compression,
};
use sha1::{Digest, Sha1};

mod tree;

use tree::Tree;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Idiot {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
#[clap(rename_all = "kebab-case")]
enum Command {
    Init,
    CatFile {
        #[arg(short)]
        print: String,
    },
    HashObject {
        #[arg(short)]
        which: String,
    },
    LsTree {
        /// Prints out only the file name. Default is `true`.
        #[arg(long)]
        name_only: bool,
        /// The sha1 of your tree.
        tree_sha: String,
    },
}

const IDIOT: &str = ".idiot";
const OBJS: &str = ".idiot/objects";
const REFS: &str = ".idiot/refs";
const HEAD: &str = ".idiot/HEAD";

/// Un-compress a Zlib Encoded vector of bytes and returns a Vec<u8> or error
fn decomp_obj(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut s = vec![];
    ZlibDecoder::new(bytes).read_to_end(&mut s)?;
    Ok(s)
}
/// Compress a vector of bytes and returns a Vec<u8> or error
fn compress_obj(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut s = vec![];
    ZlibEncoder::new(bytes, Compression::fast()).read_to_end(&mut s)?;
    Ok(s)
}

fn main() -> anyhow::Result<()> {
    let args = Idiot::parse();
    match args.command {
        Command::Init => {
            fs::create_dir(IDIOT).unwrap();
            fs::create_dir(OBJS).unwrap();
            fs::create_dir(REFS).unwrap();
            fs::write(HEAD, "ref: refs/heads/master\n").unwrap();
            println!("Initialized git directory");
        }
        Command::CatFile { print } => {
            let (dir, file) = print.split_at(2);
            let bytes = fs::read(format!("{}/{}/{}", OBJS, dir, file))
                .with_context(|| format!("no git object at '{}/{}/{}", OBJS, dir, file))?;
            let decoded = decomp_obj(&bytes).context("uncompressing object")?;
            let s = String::from_utf8_lossy(&decoded);
            print!("{}", s);
        }
        Command::HashObject { which } => {
            let bytes = fs::read(&which).with_context(|| format!("no git object at '{}", which))?;
            let encoded = compress_obj(&bytes).context("compressing object")?;
            let mut hasher = Sha1::new();
            hasher.update(&encoded);

            let sha_hash = hex::encode(hasher.finalize());
            let (dir, path) = sha_hash.split_at(2);

            match fs::create_dir(format!("{}/{}", OBJS, dir)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    anyhow::bail!(e)
                }
            }
            fs::write(format!("{}/{}/{}", OBJS, dir, path), encoded)
                .with_context(|| format!("failed to write to {}/{}/{}", OBJS, dir, path))?;
            println!("SHA: {}", sha_hash);
        }
        Command::LsTree {
            name_only,
            tree_sha,
        } => {
            let (dir, file) = tree_sha.split_at(2);
            let bytes = fs::read(format!("{}/{}/{}", OBJS, dir, file))
                .with_context(|| format!("no git object at '{}/{}/{}", OBJS, dir, file))?;
            let encoded = decomp_obj(&bytes).context("compressing object")?;
            let tree = Tree::new(&encoded);

            if name_only {
                println!(
                    "{}",
                    tree.objs
                        .iter()
                        .map(|o| o.path.clone() + "\n")
                        .collect::<String>()
                );
            } else {
                println!(
                    "{}",
                    tree.objs
                        .iter()
                        .map(|o| o.to_full_string() + "\n")
                        .collect::<String>()
                );
            }
        }
    }
    Ok(())
}
