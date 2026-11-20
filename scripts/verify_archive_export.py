"""Check PercyStar outputs for ttdb_maps_0_7.ttpf using Python zlib and Pillow."""

import argparse
import hashlib
import struct
import zlib
from pathlib import Path

from PIL import Image


def read_record(source, rows, index, expected):
    start = (rows[index][0] & 0x3FFFFFF) << 8
    boundary = (rows[index + 1][0] & 0x3FFFFFF) << 8
    stored = boundary - start - (rows[index][1] >> 20)
    if not 0 < stored <= 65536:
        raise ValueError(f"Unexpected extent for record {index}")
    source.seek(start)
    decoder = zlib.decompressobj()
    data = decoder.decompress(source.read(stored), expected + 1)
    if not decoder.eof or decoder.unused_data or decoder.unconsumed_tail or len(data) != expected:
        raise ValueError(f"Record {index} does not match a complete stream")
    return data


def decode_surface(blocks):
    # For this 256-square BC1 level each 4 KiB tile holds 32 by 16 blocks.
    linear = bytearray()
    for y in range(64):
        for x in range(64):
            address = 0
            bx, by = x & 31, y & 15
            for bit, axis in enumerate("xyyxxyxyx"):
                if axis == "x":
                    address |= (bx & 1) << bit
                    bx >>= 1
                else:
                    address |= (by & 1) << bit
                    by >>= 1
            first = (((y // 16) * 2 + x // 32) * 512 + address) * 8
            linear.extend(blocks[first:first + 8])
    return Image.frombytes("RGBA", (256, 256), bytes(linear), "bcn", 1).convert("RGB")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("record_1", type=Path)
    parser.add_argument("texture_0", type=Path)
    parser.add_argument("--dds", type=Path)
    args = parser.parse_args()
    with args.archive.open("rb") as source:
        prefix = source.read(48)
        if prefix[:8] != bytes.fromhex("52445335cfbf0000"):
            raise ValueError("Unexpected RDS5 sample")
        rows = list(struct.iter_unpack("<II", prefix[16:]))
        source.seek(((rows[0][0] & 0x3FFFFFF) << 8) + 16)
        if source.read(16) != bytes.fromhex("ffc03f60410220021001602000000000"):
            raise ValueError("Unexpected ETPF descriptor 0")
        record = read_record(source, rows, 1, 12288)
        reference = decode_surface(read_record(source, rows, 2, 32768))
    if args.record_1.stat().st_size != len(record) or args.record_1.read_bytes() != record:
        raise ValueError("Extracted record differs from Python zlib")
    with Image.open(args.texture_0) as image:
        if image.size != reference.size or image.convert("RGB").tobytes() != reference.tobytes():
            raise ValueError("Exported PNG differs from the reconstructed BC1 surface")
    print(f"{args.texture_0.name}: all {reference.width * reference.height:,} RGB pixels match")
    if args.dds:
        # Pillow's DDS header loader does not accept DXGI 72 in every release.
        # Validate the DX10 container first, then pass the declared BC1 blocks to its decoder.
        if args.dds.stat().st_size != 148 + 32768:
            raise ValueError("Unexpected DDS file length")
        data = args.dds.read_bytes()
        if (data[:4] != b"DDS " or struct.unpack_from("<5I", data, 4) != (124, 0x81007, 256, 256, 32768)
                or data[84:88] != b"DX10" or struct.unpack_from("<5I", data, 128) != (72, 3, 0, 1, 0)):
            raise ValueError("Unexpected DX10 BC1 sRGB surface header")
        decoded = Image.frombytes("RGBA", (256, 256), data[148:], "bcn", 1).convert("RGB")
        if decoded.tobytes() != reference.tobytes():
            raise ValueError("Exported DDS blocks differ from the reconstructed surface")
        print(f"{args.dds.name}: DX10 header and all RGB pixels match")
    print("Record 1 SHA-256", hashlib.sha256(record).hexdigest())
    print("Texture 0 RGB SHA-256", hashlib.sha256(reference.tobytes()).hexdigest())


if __name__ == "__main__":
    main()
