# Indexed archive browser

The **Archives** tab opens local files read-only and reads the index on a worker thread. Open a folder to list its archives by their actual filesystem paths. Expand a directory and select a file to read its index. The record list is virtualized. Ctrl+O opens an archive, Ctrl+Shift+O opens a folder, and Ctrl+F focuses the item filter. Dropping a path or passing it as the first command-line argument opens it directly. The export dialog starts in the source archive’s folder. Extraction writes a new file and rejects an existing destination.

## RDS5 and ETPF

The fixture used for local verification is `ttdb_maps_0_7.ttpf`, 1,765,679,616 bytes, SHA-256 `d672e99c248b943c3a3acb694eb16fe6236d3d9d04c86451cf68259ca68d4409`. Its signature is ASCII `RDS5`. A little-endian u32 at +0x04 counts the eight-byte boundaries beginning at 0x10. This archive has 49,103 boundaries and 49,102 data records. For successive little-endian word pairs (A, B), PercyStar uses:

| Quantity | Expression |
| --- | --- |
| Physical start | `(A & 0x03ffffff) * 256` |
| Virtual start | `((B & 0xfffff) * 4 + (A >> 30)) * 4096` |
| Stored size | Next physical start minus current physical start minus `B >> 20` |
| Decoded size | Next virtual start minus current virtual start |
| Codec tag | `(A >> 26) & 15` |

The terminal physical boundary equals EOF. The upper two physical address bits in the upstream mask are unused in this sample. The browser rejects invalid extents, impossible padding and truncated tables. Tag 0 with equal lengths is stored. Tag 8 requires a complete zlib stream with the expected checksum, consumed bytes and output length. This sample has 46,442 tag-8 records, one stored record, and 2,659 records with other tags. Unsupported tags remain visible with their physical offsets. A record can be one part of a resource.

| Index | Table row and bytes | Physical start | Stored | Decoded |
| ---: | --- | --- | ---: | ---: |
| 1 | 0x18 · `00 17 00 20 44 00 60 04` | 0x170000 | 5,818 | 12,288 |
| 2 | 0x20 · `17 17 00 E0 44 00 F0 30` | 0x171700 | 13,809 | 32,768 |

Record 1 decodes to SHA-256 `d092fd87d4a882a7b59644f39d1313ff2e0c32e2bce42491d5490226cde4b362`. Its exported filename is generated from the index. Original names and folders have not been established for this sample.

The stored metadata record begins at 0x5FF00 with `ETPF`. Its +8 u32 counts 17,773 sixteen-byte descriptors at +0x10. Descriptor 0 at 0x5FF10 is `FF C0 3F 60 41 02 20 02 10 01 60 20 00 00 00 00`. The dimensions are the low and next fourteen bits of word 0, each plus one. Word 1 bits 3–9 select DXGI format 72 for this descriptor. Masking word 2 with `0x1fffff` gives a 4 KiB virtual page. With twenty bits, only 15,740 descriptor addresses match record starts and the sequence wraps. With twenty-one bits, all 17,773 match in ascending order across a 4,856,598,528-byte virtual stream.

Descriptor 0 starts at virtual 0x110000. Its allocation covers records 1 and 2. Record 2 holds its largest 256 × 256 BC1 level. The smaller level in record 1 depicts the same texture. The 4 KiB tiles for the largest level contain 32 × 16 blocks with address bits X0 Y0 Y1 X1 X2 Y2 X3 Y3 X4. The full-resolution RGB SHA-256 is `15685339bc2f51db96a76f394bd67887017e9f494c8dff9c3c6f77dd244f4e53`. The independent verification script reads the physical rows, inflates them with Python, reorders the BC1 blocks and decodes them using Pillow. It compares every RGB pixel and the full extracted record.

## RPF0

The browser follows the little-endian RPF0 layout implemented in Swage. The byte length and entry count are at offsets 4 and 8. The directory starts at 0x800 with 16-byte rows followed by NUL-terminated names. Bit 31 of the first word identifies a folder. A folder supplies its first child and child count in words 1 and 2. A file supplies its physical offset, stored length and logical length in words 1–3. Equal lengths mean stored bytes. Unequal lengths select raw DEFLATE. The synthetic suite covers nested folders, empty directories, both encodings, unsafe names and invalid graphs. Validation on a real RPF0 is pending. [Swage entry fields](https://github.com/0x1F9F1/Swage/blob/master/src/games/rage/rpf0.h) · [Swage archive loader](https://github.com/0x1F9F1/Swage/blob/master/src/games/rage/rpf0.cpp)

## RPF9 and other inputs

`core_licenses_data.rpf` has 13,536 bytes and the `39 46 50 52` marker at 0 and 0x1000. The browser offers raw hex with absolute offsets and an explicit action to search for embedded assets in PercyStar's existing recovery tool. The words at header +0x24 remain a count hypothesis. A named member with independently known contents, or a verified directory specification, would distinguish candidate layouts. The browser exposes no RPF9 directory entries on this evidence.

Unrecognized signatures and malformed indexes still allow paged hex inspection. A malformed embedded ETPF table leaves usable RDS5 records accessible. Indexed previews read up to 512 decoded bytes for hex. Stored extraction streams to disk. Compressed records are limited to 16 MiB on each side. Texture previews accept at most 8,388,608 pixels, 8,192 per side, and 64 MiB of tiled input. PNG exports the displayed pixels. DDS preserves the linear block or pixel data of the selected largest mip. Other tile arrangements and formats implemented by the upstream viewer still need independent image comparisons. Decryption, archive editing, full mip-chain export and a universal RPF directory reader remain future work.

## Reproduce the local sample check

With Rust installed, run from this repository in PowerShell. Supply fresh output paths.

```powershell
cargo build --release --locked
cargo test --release --locked
$env:PERCYSTAR_RDS5_SAMPLE = 'D:\Samples\ttdb_maps_0_7.ttpf'
cargo test --release --locked archive_tests::local_maps_archive -- --ignored --exact
$exe = '.\target\release\PercyStar.exe'
& $exe --list-entries $env:PERCYSTAR_RDS5_SAMPLE 0 4
& $exe --extract-entry $env:PERCYSTAR_RDS5_SAMPLE 1 record-1.bin
& $exe --export-png $env:PERCYSTAR_RDS5_SAMPLE 0 texture-0.png
& $exe --export-dds $env:PERCYSTAR_RDS5_SAMPLE 0 texture-0.dds
python -m pip install Pillow
python scripts/verify_archive_export.py $env:PERCYSTAR_RDS5_SAMPLE record-1.bin texture-0.png --dds texture-0.dds
```

Optional tests need local inputs. The default suite generates its archive fixtures. For the next corpus, compare independent archives per format, retain the file and entry hashes locally, and promote a reader or preview only after a second source confirms its output.
