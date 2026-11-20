use crate::archive::{Archive, Catalog, Member, Result};
use std::path::Path;

pub fn run(args: &[String]) -> Option<Result<()>> {
    let cmd = args.get(1)?.as_str();
    if !matches!(
        cmd,
        "--list-entries" | "--extract-entry" | "--export-png" | "--export-dds" | "--help"
    ) {
        return None;
    }
    Some(dispatch(cmd, &args[2..]))
}

fn dispatch(cmd: &str, args: &[String]) -> Result<()> {
    if cmd == "--help" {
        println!("PercyStar [archive or folder]\n  --list-entries ARCHIVE [START [COUNT]]\n  --extract-entry ARCHIVE INDEX NEW_FILE\n  --export-png ARCHIVE DESCRIPTOR NEW_PNG\n  --export-dds ARCHIVE DESCRIPTOR NEW_DDS\n  --dump-textures ARCHIVE FOLDER [LIMIT]\n  --extract-models ARCHIVE FOLDER [FILTER]\n  --extract-vehicles ARCHIVE FOLDER\n  --disasm-shaders SOURCE FOLDER [LIMIT]");
        return Ok(());
    }
    if args.is_empty() || args.len() > 3 || (cmd != "--list-entries" && args.len() != 3) {
        return Err("Invalid arguments. Run PercyStar --help for usage.".into());
    }
    let archive = Archive::open(Path::new(&args[0]))?;
    let index: usize = args
        .get(1)
        .map(|s| s.parse())
        .transpose()
        .map_err(|_| "Invalid index")?
        .unwrap_or(0);
    match cmd {
        "--extract-entry" => archive
            .record(index)?
            .export(&archive.src, Path::new(&args[2])),
        "--export-png" => {
            crate::tex::export_texture_png(&archive.texture(index)?, Path::new(&args[2]))
        }
        "--export-dds" => {
            crate::tex::export_texture_dds(&archive.texture(index)?, Path::new(&args[2]))
        }
        _ => {
            let count = args
                .get(2)
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|_| "Invalid count")?
                .unwrap_or(30);
            println!("{} · {} bytes", archive.format(), archive.src.size);
            let len = match &archive.catalog {
                Catalog::Rds(ix) => {
                    println!(
                        "{} data records · {} texture descriptors",
                        ix.chunks.len(),
                        ix.descs.len()
                    );
                    if let Some(e) = &ix.descriptor_error {
                        eprintln!("Texture index unavailable: {e}");
                    }
                    ix.chunks.len()
                }
                Catalog::Rpf0(nodes) => nodes.len(),
                _ => {
                    println!("Archive directory unavailable");
                    return Ok(());
                }
            };
            println!("index\tparent\tname\tindex_row\tpayload\tstored\tdecoded\tencoding");
            for i in index..index.saturating_add(count).min(len) {
                let (name, parent, at) = match &archive.catalog {
                    Catalog::Rpf0(nodes) => {
                        let node = &nodes[i];
                        if matches!(node.member, Member::Folder(_)) {
                            println!(
                                "{i}\t{}\t{}/\t0x{:X}\t-\t-\t-\tFolder",
                                node.parent,
                                node.name,
                                0x800 + i * 16
                            );
                            continue;
                        }
                        (node.name.clone(), node.parent.to_string(), 0x800 + i * 16)
                    }
                    _ => (format!("Record {i}"), "-".into(), 16 + i * 8),
                };
                let r = archive.record(i)?;
                println!(
                    "{i}\t{parent}\t{name}\t0x{at:X}\t0x{:X}\t{}\t{}\t{}",
                    r.offset,
                    r.stored,
                    r.size,
                    r.codec.label()
                );
            }
            Ok(())
        }
    }
}
