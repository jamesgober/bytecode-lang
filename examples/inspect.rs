//! Decode an LSB file with explicit limits and print its disassembly, or
//! report exactly where and why it is malformed.
//!
//! ```text
//! cargo run --example inspect -- path/to/module.lsb
//! cargo run --example inspect            # inspects a corrupted sample
//! ```

use bytecode_lang::{Inst, Limits, ModuleBuilder, ValType, decode_with, disassemble, encode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = match std::env::args_os().nth(1) {
        Some(path) => std::fs::read(path)?,
        None => {
            // A tiny module with one byte flipped inside its code.
            let mut m = ModuleBuilder::new();
            let mut f = m.function("main", &[ValType::Dyn], &[]);
            f.emit(Inst::Throw { src: f.param(0) });
            m.add_function(f)?;
            let mut bytes = encode(&m.finish()?);
            // Corrupt the opcode byte of the `throw`.
            let word = Inst::Throw {
                src: bytecode_lang::Reg(0),
            }
            .to_bytes();
            if let Some(at) = bytes.windows(8).position(|w| w == word) {
                bytes[at] = 0xff;
            }
            bytes
        }
    };

    // A loader for untrusted files would lower these; the defaults admit
    // hundreds of megabytes of code.
    let mut limits = Limits::default();
    limits.max_bytes = 64 << 20;
    limits.max_const_depth = 32;

    match decode_with(&bytes, &limits) {
        Ok(module) => print!("{}", disassemble(&module)),
        Err(e) => {
            eprintln!("not a valid LSB module: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}
