use crate::archive::{self, Archive, Catalog, Codec, Member};
use flate2::{
    write::{DeflateEncoder, ZlibEncoder},
    Compression,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;

fn put(data: &mut [u8], at: usize, v: u32) {
    data[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn fixture(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("archive.bin");
    fs::write(&path, bytes).unwrap();
    (dir, path)
}

fn rpf0() -> Vec<u8> {
    let text = b"Named file from a synthetic RPF0 directory. ".repeat(100);
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&text).unwrap();
    let deflate = encoder.finish().unwrap();
    let names = b"\0docs\0plain.txt\0note.txt\0";
    let mut data = vec![0; 0x1004];
    data[..4].copy_from_slice(b"RPF0");
    put(&mut data, 4, (64 + names.len()) as u32);
    put(&mut data, 8, 4);
    for (i, row) in [
        [0x80000000, 1, 2, 0],
        [0x80000001, 3, 1, 0],
        [6, 0x1000, 4, 4],
        [16, 0x1004, deflate.len() as u32, text.len() as u32],
    ]
    .iter()
    .enumerate()
    {
        for (j, w) in row.iter().enumerate() {
            put(&mut data, 0x800 + 16 * i + 4 * j, *w);
        }
    }
    data[0x840..0x840 + names.len()].copy_from_slice(names);
    data[0x1000..].copy_from_slice(b"test");
    data.extend(deflate);
    data
}

fn rds() -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&vec![0x41; 4096]).unwrap();
    let zlib = encoder.finish().unwrap();
    let mut data = vec![0; 512];
    data[..4].copy_from_slice(b"RDS5");
    put(&mut data, 4, 2);
    put(&mut data, 16, 1 | (8 << 26));
    put(&mut data, 20, (256 - zlib.len() as u32) << 20);
    put(&mut data, 24, 2 | (1 << 30));
    data[256..256 + zlib.len()].copy_from_slice(&zlib);
    data
}

#[test]
fn rpf0_names_hierarchy_and_deflate_export() {
    let (dir, path) = fixture(&rpf0());
    let a = Archive::open(&path).unwrap();
    let Catalog::Rpf0(nodes) = &a.catalog else {
        panic!("wrong format")
    };
    assert_eq!(nodes[1].name, "docs");
    assert!(matches!(&nodes[0].member, Member::Folder(r) if r.clone() == (1..3)));
    assert_eq!((nodes[3].parent, nodes[3].name.as_str()), (1, "note.txt"));
    assert!(a.record(1).is_err());
    assert_eq!(a.record(2).unwrap().preview(&a.src).unwrap(), b"test");
    let out = dir.path().join("note.txt");
    a.record(3).unwrap().export(&a.src, &out).unwrap();
    assert_eq!(
        fs::read(&out).unwrap(),
        b"Named file from a synthetic RPF0 directory. ".repeat(100)
    );
    assert_eq!(fs::read(path).unwrap(), rpf0());
}

#[test]
fn rejects_cycles_unreachable_children_and_payloads_inside_the_table() {
    for (pos, value) in [
        (0x804, 0),
        (0x808, 1),
        (0x814, 4),
        (0x824, 0x801),
        (0x828, u32::MAX),
        (8, 200_000),
    ] {
        let mut bytes = rpf0();
        put(&mut bytes, pos, value);
        let (_dir, path) = fixture(&bytes);
        assert!(
            Archive::open(&path).is_err(),
            "accepted mutation at {pos:x}"
        );
    }
}

#[test]
fn rejects_unsafe_member_names() {
    for name in [
        "../a", "a/b", "dir\\x", "NUL.txt", "LPT1", "con ", "C:out", "x.", "..", "x\0y",
    ] {
        assert!(!archive::safe_component(name), "{name}");
    }
    let mut bytes = rpf0();
    bytes[0x841..0x845].copy_from_slice(b"../x");
    let (_dir, path) = fixture(&bytes);
    assert!(Archive::open(&path).is_err());
}

#[test]
fn ignored_root_name_and_empty_folders_are_supported() {
    let mut data = rpf0();
    put(&mut data, 0x800, u32::MAX);
    put(&mut data, 0x808, 3);
    put(&mut data, 0x818, 0);
    let (_dir, path) = fixture(&data);
    let a = Archive::open(&path).unwrap();
    let Catalog::Rpf0(nodes) = a.catalog else {
        panic!()
    };
    assert!(matches!(&nodes[1].member, Member::Folder(r) if r.is_empty()));
    assert_eq!(nodes[3].parent, 0);
}

#[test]
fn malformed_indexes_still_offer_read_only_hex_inspection() {
    let (_dir, path) = fixture(b"RDS5");
    let a = Archive::inspect(&path).unwrap();
    assert!(matches!(a.catalog, Catalog::Invalid("RDS5", _)));
    assert_eq!(a.src.read(0, 4).unwrap(), b"RDS5");
    assert!(a.record(0).is_err());
}

#[test]
fn rds_uses_exclusive_boundaries_and_padding() {
    let (_dir, path) = fixture(&rds());
    let a = Archive::open(&path).unwrap();
    let Catalog::Rds(ix) = &a.catalog else {
        panic!()
    };
    let r = a.record(0).unwrap();
    assert_eq!((r.offset, r.size, r.codec), (256, 4096, Codec::Zlib));
    assert!(a.record(1).is_err());
    assert_eq!(ix.read_virtual(4056, 40).unwrap(), vec![0x41; 40]);
    assert!(ix.read_virtual(4056, 41).is_err());
    assert!(ix.read_virtual(u64::MAX, 2).is_err());
    assert!(ix.read_virtual(0, 65 << 20).is_err());
}

#[test]
fn rejects_truncated_corrupt_and_trailing_zlib_bytes_without_creating_output() {
    for mutation in 0..4 {
        let mut data = rds();
        let pad = archive::word(&data, 20) >> 20;
        match mutation {
            0 => put(&mut data, 20, (pad + 1) << 20),
            1 => put(&mut data, 20, (pad - 1) << 20),
            2 => data[511 - pad as usize] ^= 0x80,
            _ => put(&mut data, 24, 2 | (2 << 30)),
        }
        let (dir, path) = fixture(&data);
        let a = Archive::open(&path).unwrap();
        let out = dir.path().join("new");
        assert!(a.record(0).unwrap().export(&a.src, &out).is_err());
        assert!(!out.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn unsupported_tag_does_not_decode_even_with_a_valid_zlib_signature() {
    let mut data = rds();
    put(&mut data, 16, 1 | (3 << 26));
    let (_dir, path) = fixture(&data);
    let a = Archive::open(&path).unwrap();
    assert_eq!(a.record(0).unwrap().codec, Codec::Unsupported(3));
    assert!(a.record(0).unwrap().decode(&a.src).is_err());
}

#[test]
fn rejects_invalid_rds_boundaries_and_huge_counts() {
    for (pos, val) in [
        (4, u32::MAX),
        (16, 0),
        (20, 257 << 20),
        (24, 1),
        (24, 3 | 1 << 30),
        (24, 2),
    ] {
        let mut data = rds();
        put(&mut data, pos, val);
        let (_dir, path) = fixture(&data);
        assert!(Archive::open(&path).is_err());
    }
    for bytes in [b"RPF0".as_slice(), b"RDS5".as_slice()] {
        let (_dir, path) = fixture(bytes);
        assert!(Archive::open(&path).is_err());
    }
}

#[test]
fn invalid_etpf_keeps_the_data_index_accessible() {
    let mut data = vec![0; 0x1100];
    data[..4].copy_from_slice(b"RDS5");
    put(&mut data, 4, 2);
    put(&mut data, 16, 1);
    put(&mut data, 24, 17 | 1 << 30);
    data[256..260].copy_from_slice(b"ETPF");
    put(&mut data, 264, u32::MAX);
    let (_dir, path) = fixture(&data);
    let a = Archive::open(&path).unwrap();
    let Catalog::Rds(ix) = &a.catalog else {
        panic!()
    };
    assert!(ix.descs.is_empty() && ix.descriptor_error.is_some());
    assert_eq!(a.record(0).unwrap().decode(&a.src).unwrap(), &data[256..]);
}

#[test]
fn virtual_pages_above_four_gib_do_not_alias_lower_pages() {
    let mut data = vec![0; 0x1300];
    data[..4].copy_from_slice(b"RDS5");
    put(&mut data, 4, 4);
    put(&mut data, 16, 1);
    put(&mut data, 24, 17 | (1 << 30) | (1 << 26)); // virtual 4096
    put(&mut data, 32, 18 | (8 << 26));
    put(&mut data, 36, 0x40000); // virtual 4 GiB
    put(&mut data, 40, 19 | (1 << 30));
    put(&mut data, 44, 0x40000);
    data[256..260].copy_from_slice(b"ETPF");
    put(&mut data, 264, 1);
    put(&mut data, 280, 0x100000);
    let (_dir, path) = fixture(&data);
    let a = Archive::open(&path).unwrap();
    let Catalog::Rds(ix) = &a.catalog else {
        panic!()
    };
    assert_eq!(
        (ix.descs[0].byte_off, ix.descs[0].alloc_sz),
        (1u64 << 32, 4096)
    );
}

#[test]
fn existing_export_source_and_atomic_collision_are_preserved() {
    let (dir, path) = fixture(&rds());
    let a = Archive::open(&path).unwrap();
    assert!(a.record(0).unwrap().export(&a.src, &path).is_err());
    assert_eq!(fs::read(&path).unwrap(), rds());
    let output = dir.path().join("new.bin");
    let result = archive::write_new(&output, |f| {
        f.write_all(b"candidate").unwrap();
        fs::write(&output, b"winner").unwrap();
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read(output).unwrap(), b"winner");
}

#[test]
fn pages_sparse_files_beyond_four_gib_without_reading_the_gap() {
    let (_dir, path) = fixture(b"9FPR");
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len((1u64 << 32) + 17).unwrap();
    drop(file);
    let a = Archive::open(&path).unwrap();
    assert!(matches!(a.catalog, Catalog::Rpf9));
    assert_eq!(a.src.read(1u64 << 32, 17).unwrap(), [0; 17]);
    assert!(a.src.read(a.src.size, 1).is_err());
}

#[test]
#[ignore = "set PERCYSTAR_RDS5_SAMPLE to the local ttdb_maps_0_7.ttpf"]
fn local_maps_archive() {
    let path = std::env::var_os("PERCYSTAR_RDS5_SAMPLE").expect("Path to the local sample");
    let a = Archive::open(Path::new(&path)).unwrap();
    let Catalog::Rds(ix) = &a.catalog else {
        panic!()
    };
    assert_eq!((ix.chunks.len(), ix.descs.len()), (49102, 17773));
    assert_eq!(
        ix.chunks
            .iter()
            .filter(|c| c.record.codec == Codec::Zlib)
            .count(),
        46442
    );
    let r = a.record(1).unwrap();
    assert_eq!((r.offset, r.stored, r.size), (0x170000, 5818, 12288));
    assert_eq!(
        format!("{:x}", Sha256::digest(r.decode(&a.src).unwrap())),
        "d092fd87d4a882a7b59644f39d1313ff2e0c32e2bce42491d5490226cde4b362"
    );
    let tex = a.texture(0).unwrap();
    assert_eq!((tex.w, tex.h, tex.dxgi), (256, 256, 72));
    let rgb: Vec<u8> = tex
        .pixels
        .iter()
        .flat_map(|p| [p.r(), p.g(), p.b()])
        .collect();
    assert_eq!(
        format!("{:x}", Sha256::digest(rgb)),
        "15685339bc2f51db96a76f394bd67887017e9f494c8dff9c3c6f77dd244f4e53"
    );
}
