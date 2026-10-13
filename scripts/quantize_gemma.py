#!/usr/bin/env python3
"""Convert local F32/F16 GGUF matrices to GGML Q4_0; keep vectors in full precision.

Requires NumPy. Writes a separate GGUF and Forge image, preserving the source.
"""
import argparse
import hashlib
import math
import mmap
import os
from pathlib import Path
import struct

import numpy as np


def convert(source, output):
    if source.resolve() == output.resolve() or output.exists() or output.with_suffix('.img').exists():
        raise ValueError('Choose a new output path; existing files are never overwritten')
    with source.open('rb') as file:
        data = mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_READ)
        base = 0 if data[:4] == b'GGUF' else 4096
        if data[base:base + 4] != b'GGUF':
            raise ValueError('Input is not GGUF or a Forge image')
        position = base + 4

        def read(fmt):
            nonlocal position
            result = struct.unpack_from('<' + fmt, data, position)[0]
            position += struct.calcsize('<' + fmt)
            return result

        def string():
            nonlocal position
            length = read('Q')
            text = data[position:position + length].decode('utf8')
            position += length
            return text

        def value(kind):
            formats = {0: 'B', 1: 'b', 2: 'H', 3: 'h', 4: 'I', 5: 'i',
                       6: 'f', 7: 'B', 10: 'Q', 11: 'q', 12: 'd'}
            if kind in formats:
                return read(formats[kind])
            if kind == 8:
                return string()
            if kind == 9:
                item_kind, count = read('I'), read('Q')
                for _ in range(count):
                    value(item_kind)
                return None
            raise ValueError(f'Unsupported metadata type {kind}')

        if read('I') not in (2, 3):
            raise ValueError('Unsupported GGUF version')
        tensor_count, metadata_count = read('Q'), read('Q')
        alignment, file_type_position = 32, None
        for _ in range(metadata_count):
            key, kind = string(), read('I')
            start = position
            item = value(kind)
            if key == 'general.alignment':
                alignment = item
            if key == 'general.file_type':
                if kind != 4:
                    raise ValueError('Unexpected general.file_type encoding')
                file_type_position = start - base
        tensors = []
        for _ in range(tensor_count):
            name = string()
            shape = [read('Q') for _ in range(read('I'))]
            type_position = position - base
            kind, offset = read('I'), read('Q')
            if kind not in (0, 1):
                raise ValueError('Input must contain only F32/F16 weights')
            tensors.append((name, shape, kind, offset, type_position))
        if alignment < 1 or alignment > 4096 or alignment & (alignment - 1):
            raise ValueError('Invalid GGUF alignment')
        data_start = (position - base + alignment - 1) // alignment * alignment
        header = bytearray(data[base:base + data_start])
        if file_type_position is not None:
            struct.pack_into('<I', header, file_type_position, 2)  # MOSTLY_Q4_0
        output.parent.mkdir(parents=True, exist_ok=True)
        partial = output.with_suffix(output.suffix + '.part')
        with partial.open('wb') as destination:
            destination.write(header)
            for name, shape, kind, offset, type_position in tensors:
                destination.write(bytes((-destination.tell()) % alignment))
                new_offset = destination.tell() - data_start
                count = math.prod(shape)
                dtype = '<f4' if kind == 0 else '<f2'
                start = base + data_start + offset
                size = count * (4 if kind == 0 else 2)
                if start + size > len(data):
                    raise ValueError(f'Truncated tensor {name}')
                quantized = len(shape) >= 2 and shape[0] % 32 == 0
                struct.pack_into('<IQ', header, type_position, 2 if quantized else kind, new_offset)
                if not quantized:
                    destination.write(data[start:start + size])
                    continue
                # Bounded chunks prevent another whole-model RAM allocation.
                for first in range(0, count, 32 * 8192):
                    length = min(32 * 8192, count - first)
                    weights = np.frombuffer(data, dtype=dtype, count=length,
                                            offset=start + first * (4 if kind == 0 else 2)).astype(np.float32).reshape(-1, 32)
                    if not np.isfinite(weights).all():
                        raise ValueError(f'Nonfinite weights in {name}')
                    maximum = weights[np.arange(len(weights)), np.abs(weights).argmax(axis=1)]
                    scale = maximum / np.float32(-8)
                    inverse = np.divide(1, scale, out=np.zeros_like(scale), where=scale != 0)
                    q = np.clip(np.trunc(weights * inverse[:, None] + np.float32(8.5)), 0, 15).astype(np.uint8)
                    blocks = np.empty((len(weights), 18), dtype=np.uint8)
                    blocks[:, :2] = scale.astype('<f2').view(np.uint8).reshape(-1, 2)
                    blocks[:, 2:] = q[:, :16] | (q[:, 16:] << 4)
                    destination.write(blocks.tobytes())
                print(f'Quantized {name}', flush=True)
            destination.seek(0)
            destination.write(header)
            destination.flush()
            os.fsync(destination.fileno())
        data.close()
        os.replace(partial, output)
    digest = hashlib.sha256()
    with output.open('rb') as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    from download_gemma import package
    image = output.with_suffix('.img')
    if image.exists():
        raise ValueError('Image output already exists')
    package(output, image, digest.hexdigest(), output.stat().st_size)
    print(f'GGUF: {output}; Forge image: {image}; bytes: {output.stat().st_size}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    convert(args.source, args.output)
