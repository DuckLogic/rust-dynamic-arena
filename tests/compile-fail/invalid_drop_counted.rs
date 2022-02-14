extern crate dynamic_arena;

use dynamic_arena::DynamicArena;
use std::cell::Cell;

pub struct DropCounted<'a>(&'a Cell<u32>);

fn main() {
    let arena = DynamicArena::new_bounded();
    {
        let cell = Box::new(Cell::new(0));
        for _i in 0..5 {
            arena.alloc(DropCounted(&cell));
        }
    }
    /*
     * Running this drop would reference an invalid pointer to the stack allocated `Cell`.
     * In order to maintain rust's safety guarantee this must be statically prevented.
     */
    drop(arena);
}
