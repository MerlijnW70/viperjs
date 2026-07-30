//! §25.1 — an `ArrayBuffer`'s bytes, and what it means for one to be detached.
//!
//! # Why the bytes are a `Vec<u8>` and not a `Vec<Value>`
//!
//! Because that is what the specification says they are. §25.1.3.1's data block is a sequence of
//! *bytes*, and everything above it — every TypedArray, every `DataView` — is a way of reading and
//! writing those bytes with a type imposed on the way past. A buffer holds no JavaScript values at
//! all, which is why it needs nothing from the collector: there is nothing in it to keep alive.
//!
//! # Detaching
//!
//! §25.1.3.3 `DetachArrayBuffer` throws the bytes away and leaves the object behind. Every view
//! onto it then throws on every access, and `byteLength` answers 0. It is not a way of freeing
//! memory that a program can rely on; it is what `transfer` and the host's own APIs do when the
//! bytes have gone somewhere else, and it is what makes "is this buffer still usable" a question a
//! specification has to ask before every read.
//!
//! `None` is detached and `Some(vec![])` is an empty buffer that is perfectly usable. The two are
//! different in every way that matters and identical in `byteLength`, which is exactly the sort of
//! distinction an `Option` is for.

/// §25.1.3.1's data block — the bytes, or nothing if the buffer has been detached.
#[derive(Debug)]
pub struct Buffer {
    /// `[[ArrayBufferData]]`, or `None` once `[[ArrayBufferDetachKey]]` has done its work.
    bytes: Option<Vec<u8>>,
}

impl Buffer {
    /// A buffer of `length` **zeroed** bytes — §25.1.3.1 step 2.
    ///
    /// Zeroed and not uninitialised, which is not an implementation detail: a program can read
    /// every byte of a fresh buffer and the answer has to be 0. Anything else would hand it
    /// whatever was in that memory before.
    #[must_use]
    pub fn new(length: usize) -> Self {
        Self {
            bytes: Some(vec![0; length]),
        }
    }

    /// The bytes, or `None` if this buffer is detached.
    #[must_use]
    pub fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }

    /// The same, to write through.
    pub fn bytes_mut(&mut self) -> Option<&mut [u8]> {
        self.bytes.as_deref_mut()
    }

    /// `[[ArrayBufferByteLength]]` — **0** for a detached buffer, which §25.1.5.1 is explicit about.
    #[must_use]
    pub fn byte_length(&self) -> usize {
        self.bytes.as_ref().map_or(0, Vec::len)
    }

    /// Whether the bytes have gone — §25.1.3.2 `IsDetachedBuffer`.
    #[must_use]
    pub fn detached(&self) -> bool {
        self.bytes.is_none()
    }

    /// §25.1.3.3 `DetachArrayBuffer` — throw the bytes away and leave the object.
    pub fn detach(&mut self) {
        self.bytes = None;
    }
}

/// §25.3's `DataView` slots — which buffer, where in it, and how much of it.
///
/// A view is a *window*: it holds no bytes of its own, and two views over one buffer see each
/// other's writes. The offset and the length are fixed when the view is made, which is what makes
/// a view a promise about a region rather than a pointer into one.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// `[[ViewedArrayBuffer]]`.
    pub buffer: crate::heap::ObjectId,
    /// `[[ByteOffset]]` — where the window starts.
    pub offset: usize,
    /// `[[ByteLength]]` — how wide it is.
    pub length: usize,
}
