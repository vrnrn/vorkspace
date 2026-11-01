// Upstream's fixture needs a C helper crate that isn't published. These build the
// same blocks through this crate, so copy, call and disposal still run for real.
use {Block, ConcreteBlock, RcBlock};

pub fn get_int_block_with(i: i32) -> RcBlock<(), i32> {
    ConcreteBlock::new(move || i).copy()
}

pub fn get_add_block_with(i: i32) -> RcBlock<(i32,), i32> {
    ConcreteBlock::new(move |a: i32| a + i).copy()
}

pub fn invoke_int_block(block: &Block<(), i32>) -> i32 {
    unsafe { block.call(()) }
}

pub fn invoke_add_block(block: &Block<(i32,), i32>, a: i32) -> i32 {
    unsafe { block.call((a,)) }
}
