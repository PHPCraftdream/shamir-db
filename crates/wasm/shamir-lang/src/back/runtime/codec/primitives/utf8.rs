use wasm_encoder::{BlockType, Function, Instruction, MemArg, ValType};

/// `(ptr:i32,len:i32) -> i32`; valid UTF-8 returns 1, malformed input 0.
pub(crate) fn emit_validate_utf8() -> Function {
    let mut function = Function::new([(6, ValType::I32)]);
    macro_rules! i { ($op:ident $(, $arg:expr)?) => { function.instruction(&Instruction::$op $(($arg))?); }; }

    i!(LocalGet, 0);
    i!(LocalGet, 1);
    i!(I32Add);
    i!(LocalSet, 2);
    i!(Block, BlockType::Empty);
    i!(Loop, BlockType::Empty);
    i!(LocalGet, 0);
    i!(LocalGet, 2);
    i!(I32GeU);
    i!(BrIf, 1);
    i!(LocalGet, 0);
    i!(
        I32Load8U,
        MemArg {
            offset: 0,
            align: 0,
            memory_index: 0
        }
    );
    i!(LocalSet, 3);
    i!(LocalGet, 0);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, 0);
    i!(LocalGet, 3);
    i!(I32Const, 128);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(Else);
    i!(LocalGet, 3);
    i!(I32Const, 194);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(LocalGet, 3);
    i!(I32Const, 244);
    i!(I32GtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(LocalGet, 3);
    i!(I32Const, 224);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 1);
    i!(LocalSet, 4);
    i!(LocalGet, 3);
    i!(I32Const, 31);
    i!(I32And);
    i!(LocalSet, 5);
    i!(I32Const, 128);
    i!(LocalSet, 6);
    i!(Else);
    i!(LocalGet, 3);
    i!(I32Const, 240);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 2);
    i!(LocalSet, 4);
    i!(LocalGet, 3);
    i!(I32Const, 15);
    i!(I32And);
    i!(LocalSet, 5);
    i!(I32Const, 2048);
    i!(LocalSet, 6);
    i!(Else);
    i!(I32Const, 3);
    i!(LocalSet, 4);
    i!(LocalGet, 3);
    i!(I32Const, 7);
    i!(I32And);
    i!(LocalSet, 5);
    i!(I32Const, 65536);
    i!(LocalSet, 6);
    i!(End);
    i!(End);
    i!(LocalGet, 2);
    i!(LocalGet, 0);
    i!(I32Sub);
    i!(LocalGet, 4);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(Block, BlockType::Empty);
    i!(Loop, BlockType::Empty);
    i!(LocalGet, 4);
    i!(I32Eqz);
    i!(BrIf, 1);
    i!(LocalGet, 0);
    i!(
        I32Load8U,
        MemArg {
            offset: 0,
            align: 0,
            memory_index: 0
        }
    );
    i!(LocalSet, 7);
    i!(LocalGet, 7);
    i!(I32Const, 128);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(LocalGet, 7);
    i!(I32Const, 191);
    i!(I32GtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(LocalGet, 5);
    i!(I32Const, 6);
    i!(I32Shl);
    i!(LocalGet, 7);
    i!(I32Const, 63);
    i!(I32And);
    i!(I32Or);
    i!(LocalSet, 5);
    i!(LocalGet, 0);
    i!(I32Const, 1);
    i!(I32Add);
    i!(LocalSet, 0);
    i!(LocalGet, 4);
    i!(I32Const, 1);
    i!(I32Sub);
    i!(LocalSet, 4);
    i!(Br, 0);
    i!(End);
    i!(End);
    i!(LocalGet, 5);
    i!(LocalGet, 6);
    i!(I32LtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(LocalGet, 5);
    i!(I32Const, 1114111);
    i!(I32GtU);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(LocalGet, 5);
    i!(I32Const, 55296);
    i!(I32GeU);
    i!(LocalGet, 5);
    i!(I32Const, 57343);
    i!(I32LeU);
    i!(I32And);
    i!(If, BlockType::Empty);
    i!(I32Const, 0);
    i!(Return);
    i!(End);
    i!(End);
    i!(Br, 0);
    i!(End);
    i!(End);
    i!(I32Const, 1);
    i!(End);
    function
}
