#!/usr/bin/env python3
"""RUSTC_WRAPPER: build the `vmx` crate with A32 NEON enabled.

`armv7-unknown-linux-gnueabi` is a soft-float target and LLVM either errors
("Do not know how to soften this operator's operand") or crashes in
InstrEmitter::EmitRegSequence when codegen'ing the A32 NEON kernels.
Disabling the `soft-float` target feature (hardware VFP instead of soft-float
emulation) makes codegen succeed; VMX1 itself is integer-only, so no float
values cross the crate boundary.
"""
import os
import sys

argv = sys.argv[1:]
if "--crate-name" in argv:
    i = argv.index("--crate-name")
    if i + 1 < len(argv) and argv[i + 1] == "vmx":
        argv = argv + ["-C", "target-feature=-soft-float,+neon"]
os.execv(argv[0], argv)
