//! Implements dynamically typed arenas, where any type of item can be allocated.
#![deny(missing_docs)]
use std::marker::PhantomData;
use std::cell::RefCell;
use std::os::raw::c_void;
use std::mem;
use std::ptr::{self, NonNull};
use std::alloc::Layout;

use bumpalo::Bump;

/// Marker trait that indicates whether or a `DynamicArena` may be sent across threads
pub trait SendAbility: Sized {
    /// Create an arena corresponding to this type of thread-safety
    fn create_arena<'a>() -> DynamicArena<'a, Self>;
}
/// Marker type that indicates you expect everything in the `DynamicArena` to be `Send`
///
/// Although this prevents you from allocating non-`Send` types in the arena,
/// it allows you to `Send` the dynamic itself arena across threads.
/// We can't safely implement `Send` for `DynamicArena` without this bound,
/// since you could otherwise place a `Rc` in the arena, send it across threads,
/// and then proceed to drop the arena and mutate the reference count.
pub struct Sendable {
    _marker: ()
}
impl SendAbility for Sendable {
    #[inline]
    fn create_arena<'a>() -> DynamicArena<'a, Self> {
        DynamicArena::new_send()
    }
}
/// Marker type that indiates everything in the `DynamicArena` isn't nesicarrily `Send`.
///
/// This prevents you from `Send`ing the arena itself across threads,
/// as described in the `Sendable` docs.
pub struct NonSend {
    _marker: std::rc::Rc<()>
}
impl SendAbility for NonSend {
    #[inline]
    fn create_arena<'a>() -> DynamicArena<'a, Self> {
        DynamicArena::new_bounded()
    }
}

struct DynamicArenaItem {
    drop: unsafe fn(*mut c_void),
    value: *mut c_void
}
impl Drop for DynamicArenaItem {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            (self.drop)(self.value)
        }
    }
}
unsafe impl Send for DynamicArenaItem {}

/// An alias for an arena allocator which requires that everything is `Send + 'a`.
pub type DynamicSendArena<'a> = DynamicArena<'a, Sendable>;

/// An arena allocator where any type of object can be allocated.
///
/// Unlike typed arenas, any `Sized` object can be allocated here and they
/// don't all have to have the same statically known type in advance.
/// Usually you don't have to worry about the arena's lifetime,
/// since it should be static by default (see).
///
/// ## Performance
/// Although this is _slightly_ slower than a `typed_arena::Arena`,
/// it can be much more memory and time efficient than having a ton of seperate typed arenas.
/// The only point where dynamic dispatch actually gets involved is when the arena is dropped,
/// since we have to dynamically dispatch the drop functions instead of statically dispatching them.
///
/// ## Safety
/// In order to prevent use after free in a `DynamicArena`, all pointers in the allocated items
/// need to be valid for the lifetime `'a` to ensure all references outlive the arena itself.
/// Unfortunately, this statically prevents all self referential structs with `alloc`,
/// since they can't be known ahead of time to be safe and outlive the arena itself,
/// and we can't perform [dropchk](https://doc.rust-lang.org/nightly/nomicon/dropck.html)
/// on a dynamically typed arena (only statically typed ones).
///
/// The alternatives to this are `alloc_unchecked`, which bypasses the lifetime and safety,
/// and `alloc_copy`, which bypasses the lifetime by ensuring `T: Copy`.
/// This is safe, since a copyable item can never have a custom drop,
/// and the drop function could never possibly trigger use after free.
///
/// This means you can use self-referential structs with a `DynamicArena` as long as they implement `Copy`.
/// One way to make your types implement copy and support self-refrential structs,
/// is by replacing owned objects with their borrowed counterparts and then allocating them in the arena.
/// For example
/// ````
/// struct OwnedSelfReferential<'a> {
///    next: Option<&'a OwnedSelfReferential<'a>>,
///    text: String,
///    array: Vec<u32>
/// }
/// ````
/// can't be used with either `alloc` (since it's self referential),
/// nor `alloc_copy` (since `String` and `Vec` need to be dropped).
/// However by replacing `String` with `&'a str` and `&'a [u32]`,
/// we can make the structure `Copy` and enable use with `alloc_copy`.
///
/// Then, when someone needs to arena-allocate the struct they can use
/// the same arena to allocate the `String` and `Vec<u32>` first,
/// before they proceed to allocate the copyable struct.
pub struct DynamicArena<'a, S = NonSend> {
    /// The underlying arena, where we request that they allocate arbitrary bytes.
    handle: Bump,
    /// The list of untyped values we've allocated in the arena,
    /// and whose drop functions need to be invoked.
    ///
    /// The drop functions are dynamically dispatched,
    /// and each item could invoke completely different code for completely different types.
    /// This is only needed for types that need to be dropped (as determined by `mem::needs_drop`),
    /// and types that need need to be dropped don't need to be added.
    items: RefCell<Vec<DynamicArenaItem>>,
    /// This is the magic `PhantomData` combination to have proper lifetime invariance.
    ///
    /// Otherwise the lifetime would be 'variant',
    /// and borrow checking wouldn't properly enforce the bound for `alloc`.
    /// This is enforced by the `compile-fail/invalid-drop-counted.rs`
    marker: PhantomData<*mut &'a ()>,
    send: PhantomData<S>
}
impl DynamicArena<'static, NonSend> {
    /// Create a new dynamic arena whose allocated items must outlive the `'static` lifetime,
    /// and whose items aren't required to be `Send`.
    ///
    /// Usually this is what you want,
    /// since it's only a bound for the allocated items.
    #[inline]
    pub fn new() -> Self {
        DynamicArena::new_bounded()
    }
}
impl<'a, S> DynamicArena<'a, S> {
    /// Create an arena with pre-allocated capacity for the specified number of items
    /// and bytes.
    ///
    /// NOTE: The "item" capacity excludes `Copy` references that
    /// don't need to be dropped.
    pub fn with_capacity(item_capacity: usize, byte_capacity: usize) -> Self {
        DynamicArena {
            handle: Bump::with_capacity(byte_capacity),
            items: RefCell::new(Vec::with_capacity(item_capacity)),
            marker: PhantomData,
            send: PhantomData
        }
    }
    /// Allocate the specified value in this arena,
    /// returning a reference which will be valid for the lifetime of the entire arena.
    ///
    /// The bound on the item requires that `T: Copy`
    /// to ensure there's no drop function that needs to be invoked.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc_copy<T: Copy + Send>(&self, value: T) -> &mut T {
        unsafe {
            self.alloc_unchecked(value)
        }
    }
    /// Allocate the specified value in this arena,
    /// without calling its `Drop` function.
    ///
    /// Since this doesn't call the 'Drop' impleentation,
    /// this function leaks the underlying memory.
    ///
    /// ## Safety
    /// Technically, this function is safe to use.
    /// However, it leaks memory unconditionally (without calling Drop).
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn alloc_unchecked<T>(&self, value: T) -> &mut T {
        let ptr = self.alloc_layout(Layout::new::<T>()).as_ptr().cast::<T>();
        ptr.write(value);
        &mut *ptr
    }
    /// Allocate space for an object with the specified layout
    ///
    /// The returned pointer points at uninitialized memory
    ///
    /// ## Safety
    /// Technically, only the use of the memory is unsafe.
    ///
    /// It would theoretically be possible to mark this function safe,
    /// just like [Bump::alloc_layout].
    #[inline]
    pub unsafe fn alloc_layout(&self, layout: Layout) -> NonNull<u8> {
        self.handle.alloc_layout(layout)
    }
    /// Dynamically drop the specified value,
    /// invoking the drop function when the arena is dropped.
    ///
    /// ## Safety
    /// This assumes it's safe to drop the value at the same time the arena is dropped.
    /// Not only are you assuming that [ptr::drop_in_place] would be safe,
    /// you're also assuming that the drop function won't reference any dangling pointers,
    /// and that [dropchk](https://doc.rust-lang.org/nomicon/dropck.html) would be successful.
    ///
    /// Normally these invariants are statically checked by the `alloc` method,
    /// which ensures that the memory is owned and all pointers
    /// would be valid for the lifetime of the entire arena.
    #[inline]
    pub unsafe fn dynamic_drop<T>(&self, value: *mut T) {
        if mem::needs_drop::<T>() {
            self.items.borrow_mut().push(DynamicArenaItem {
                drop: mem::transmute::<unsafe fn(*mut T),
                    unsafe fn(*mut c_void)>(ptr::drop_in_place::<T>),
                value: value as *mut c_void
            })
        }
    }
    /// Retrieve the underlying [bump allocator](bumpalo::Bump) for this arena
    #[inline]
    pub fn as_bumpalo(&self) -> &'_ bumpalo::Bump {
        &self.handle
    }
}
impl<'a> DynamicArena<'a, Sendable> {
    /// Create a new empty arena, bounded by the inferred lifetime for this type `'a`
    ///
    /// Since this arena has been marked `Sendable`,
    /// all items in the arena need to implement `Send`.
    pub fn new_send() -> Self {
        DynamicArena {
            handle: Bump::new(),
            items: RefCell::new(Vec::new()),
            marker: PhantomData,
            send: PhantomData
        }
    }
    /// Allocate the specified value in this arena,
    /// returning a reference which will be valid for the lifetime of the entire arena.
    ///
    /// The bound on this item requires that `T: 'a`
    /// to ensure the drop function is safe to invoke.
    /// Additionally, since the arena is `Sendable`,
    /// the bound on the item also requires that `T: Send`.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc<T: Send + 'a>(&self, value: T) -> &mut T {
        unsafe {
            let target = self.alloc_unchecked(value);
            self.dynamic_drop(target);
            target
        }
    }
}
impl<'a> DynamicArena<'a, NonSend> {
    /// Create a new empty arena, bounded by the inferred lifetime for this type `'a`
    ///
    /// Since this arena has been marked `NonSend`,
    /// the items in the arena don't necessarily need to implement `Send`.
    pub fn new_bounded() -> Self {
        DynamicArena {
            handle: Bump::new(),
            items: RefCell::new(Vec::new()),
            marker: PhantomData,
            send: PhantomData
        }
    }
    /// Allocate the specified value in this arena,
    /// returning a reference which will be valid for the lifetime of the entire arena.
    ///
    /// The bound on this item requires that `T: 'a`
    /// to ensure the drop function is safe to invoke.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn alloc<T: 'a>(&self, value: T) -> &mut T {
        unsafe {
            let target = self.alloc_unchecked(value);
            self.dynamic_drop(target);
            target
        }
    }
}
impl<'a, S: SendAbility> Default for DynamicArena<'a, S> {
    #[inline]
    fn default() -> Self {
        S::create_arena()
    }
}
unsafe impl<'a, S: SendAbility + Send> Send for DynamicArena<'a, S> {}
impl<'a, S> Drop for DynamicArena<'a, S> {
    #[inline]
    fn drop(&mut self) {
        // Items must be dropped before the arena
        self.items.get_mut().clear();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::cell::Cell;

    const EXPECTED_DROP_COUNT: u32 = 4787;
    const EXPECTED_DEPTHS: &[u32] = &[5, 27, 43];

    struct DropCounted<'a>(&'a Cell<u32>);
    impl<'a> Drop for DropCounted<'a> {
        fn drop(&mut self) {
            let old_count = self.0.get();
            self.0.set(old_count + 1);
        }
    }
    #[derive(Copy, Clone)]
    pub struct SelfReferential<'a>(u32, Option<&'a SelfReferential<'a>>);
    impl<'a> SelfReferential<'a> {
        #[inline]
        pub fn with_depth(arena: &'a DynamicArena, depth: u32) -> &'a Self {
            arena.alloc_copy(match depth {
                0 => SelfReferential(depth, None),
                _ => SelfReferential(depth, Some(
                    SelfReferential::with_depth(arena, depth - 1)))
            })
        }
        #[inline]
        pub fn depth(&self) -> u32 {
            match self.1 {
                Some(inner) => inner.depth() + 1,
                None => 0
            }
        }
    }
    #[test]
    fn test_send() {
        let arena = DynamicArena::new_send();
        verify_copyable(do_copyable(&arena));
        ::std::thread::spawn(move || {
            verify_copyable(do_copyable(&arena));
        });
    }

    #[test]
    fn copyable() {
        let arena = DynamicArena::new();
        for _ in 0..5 {
            verify_copyable(do_copyable(&arena));
        }
    }
    #[test]
    fn self_referential() {
        let arena = DynamicArena::new();
        for _ in 0..5 {
            verify_self_referential(do_self_referential(&arena));
        }
    }
    #[test]
    fn drop_counted() {
        let cell = Box::new(Cell::new(0));
        let arena = DynamicArena::new_bounded();
        {
            do_drop_counted(&arena, &cell);
            assert_eq!(cell.get(), 0);
        }
        drop(arena);
        assert_eq!(cell.get(), EXPECTED_DROP_COUNT);
    }
    #[test]
    fn mixed() {
        let cell = Cell::new(0);
        let arena = DynamicArena::new_bounded();
        {
            do_drop_counted(&arena, &cell);
            for _ in 0..5 {
                verify_copyable(do_copyable(&arena));
                verify_self_referential(do_self_referential(&arena));
            }
            assert_eq!(cell.get(), 0);
        }
        drop(arena);
        assert_eq!(cell.get(), EXPECTED_DROP_COUNT);
    }
    fn do_copyable<'a, S>(arena: &'a DynamicArena<S>) -> Vec<&'a u32> {
        let mut results = Vec::new();
        for i in 0..10 {
            results.push(&*arena.alloc_copy(i * 3));
        }
        results
    }
    fn verify_copyable(results: Vec<&u32>) {
        for (actual, expected) in results.iter().zip(0..10) {
            assert_eq!(**actual, expected * 3);
        }
    }
    fn do_drop_counted<'a, 'd: 'a>(arena: &'a DynamicArena<'d>, counter: &'d Cell<u32>) {
        for _ in 0..EXPECTED_DROP_COUNT {
            arena.alloc(DropCounted(counter));
        }
    }
    fn do_self_referential<'a>(arena: &'a DynamicArena) -> Vec<&'a SelfReferential<'a>> {
        let mut result = Vec::new();
        for &depth in EXPECTED_DEPTHS {
            result.push(SelfReferential::with_depth(arena, depth));
        }
        result
    }
    fn verify_self_referential<'a>(results: Vec<&'a SelfReferential<'a>>) {
        for (&actual, &depth) in results.iter().zip(EXPECTED_DEPTHS.iter()) {
            assert_eq!(actual.depth(), depth);
        }
    }
}
