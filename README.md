rust-dynamic-arena
===================
Dynamically typed arenas, supporting any `Sized` type.

## Features
- Arena allocation is very fast, just requiring a pointer-bump in the common case
- You can statically guarantee the pointers will live as long as the arena
  - Unfortunately this means the arena's memory can only be freed all at once
- Allows creating self referential arena-allocated structs
- Instead of using multiple `typed_arena::Arena`s, you can use one `DynamicArena`
  - This helps reason about your code and significantly reduce memory usage
  - Since all the memory is contiguous, it could even help improve cache performance.

## Disadvantages
- Slightly slower allocation than a `typed_arena::Arena` for non-`Copy` types
  - This is because dropping is dynamically dispatched
-  In order to maintain safety, all pointers in the allocated items
   must live at longer as long as the arena itself,
   so that the items do not outlive the items they point to.
  - This statically prevents using self-referential structs with `alloc`
  - However, you can still use them with `alloc_copy`
    - See the safety section of the docs for more info on how to use this