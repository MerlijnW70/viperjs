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

/// The eight numeric types §25.1.3.1's bytes can be read as.
///
/// Shared by §25.3's `DataView`, which asks for one per access, and by §23.2's TypedArrays, which
/// each have one for their whole length. That is the only difference between the two: a `DataView`
/// is a window with no type and a TypedArray is a window with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    /// A signed byte.
    Int8,
    /// An unsigned byte.
    Uint8,
    /// A signed 16-bit integer.
    Int16,
    /// An unsigned 16-bit integer.
    Uint16,
    /// A signed 32-bit integer.
    Int32,
    /// An unsigned 32-bit integer.
    Uint32,
    /// An IEEE single.
    Float32,
    /// An IEEE double.
    Float64,
}

impl Element {
    /// How many bytes one of these takes — §25.3.1.1's element size.
    #[must_use]
    pub fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    /// The name §25.3.4 gives the pair of methods that read and write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8",
            Self::Uint8 => "Uint8",
            Self::Int16 => "Int16",
            Self::Uint16 => "Uint16",
            Self::Int32 => "Int32",
            Self::Uint32 => "Uint32",
            Self::Float32 => "Float32",
            Self::Float64 => "Float64",
        }
    }

    /// §25.3.1.3 `RawBytesToNumeric` — the bytes, already in reading order, as a Number.
    #[must_use]
    pub fn read(self, bytes: &[u8]) -> f64 {
        let mut eight = [0_u8; 8];
        eight[..bytes.len()].copy_from_slice(bytes);
        match self {
            Self::Int8 => f64::from(eight[0] as i8),
            Self::Uint8 => f64::from(eight[0]),
            Self::Int16 => f64::from(i16::from_le_bytes([eight[0], eight[1]])),
            Self::Uint16 => f64::from(u16::from_le_bytes([eight[0], eight[1]])),
            Self::Int32 => f64::from(i32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            Self::Uint32 => f64::from(u32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            // §6.1.6.1's Number is a double, so a float32 widens on the way out. Every float32 is
            // exactly representable as a double, so nothing is lost — but the *value* is the
            // rounded one, which is why `v.setFloat32(0, 0.1); v.getFloat32(0)` is not `0.1`.
            Self::Float32 => {
                f64::from(f32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]]))
            }
            Self::Float64 => f64::from_le_bytes(eight),
        }
    }

    /// §25.3.1.5 `NumericToRawBytes` — a Number as bytes, in little-endian order.
    ///
    /// The integer conversions are `ToIntN`/`ToUintN` (§7.1.7 and following), which **wrap** rather
    /// than clamp or throw: `setUint8(0, 256)` writes 0, and `setInt8(0, 200)` writes -56. That is
    /// modular arithmetic and not saturation, and it is what makes a TypedArray of bytes behave
    /// like memory rather than like a checked container.
    #[must_use]
    pub fn write(self, value: f64) -> Vec<u8> {
        match self {
            Self::Int8 | Self::Uint8 => vec![wrap(value, 8) as u8],
            Self::Int16 | Self::Uint16 => (wrap(value, 16) as u16).to_le_bytes().to_vec(),
            Self::Int32 | Self::Uint32 => (wrap(value, 32) as u32).to_le_bytes().to_vec(),
            Self::Float32 => (value as f32).to_le_bytes().to_vec(),
            Self::Float64 => value.to_le_bytes().to_vec(),
        }
    }
}

/// §7.1.7 `ToIntN`/`ToUintN` — a Number as `bits` bits, wrapping.
///
/// `NaN`, both infinities and every fractional part go to zero or are truncated first (§7.1.5), and
/// only then does the value wrap. So `setUint8(0, NaN)` writes 0 rather than refusing, which is
/// what makes writing to a buffer total.
fn wrap(value: f64, bits: u32) -> u64 {
    let truncated = value.trunc();
    let modulus = 2_f64.powi(bits as i32);
    // `rem_euclid` rather than `%`, because `%` keeps the sign of the left operand and this has to
    // answer a *non-negative* residue: -1 as a byte is 255 and not -1.
    let wrapped = truncated.rem_euclid(modulus);
    // `NaN` and both infinities arrive here as `NaN` — `inf.rem_euclid(256)` is `NaN` — and a
    // `f64 as u64` cast **saturates**: `NaN` becomes 0, as does anything negative. So the guard
    // §7.1.5 writes out is already in the cast, and written twice it was a branch nothing could
    // tell from its absence.
    wrapped as u64
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
    /// The type its bytes are read as, or `None` for a `DataView`.
    ///
    /// The whole difference between §25.3 and §23.2: a `DataView` is a window with no type, which
    /// asks for one at every access, and a TypedArray is a window that has one for its whole
    /// length. Everything else about them — the buffer, the offset, the width, the detaching — is
    /// the same, so they are the same record with this field answered differently.
    pub element: Option<Element>,
}

impl View {
    /// How many elements this window holds — its byte length over its element's width.
    ///
    /// Zero for a `DataView`, which has no elements at all: `length` there is a count of *bytes*,
    /// and §25.3 never divides it.
    #[must_use]
    pub fn count(&self) -> usize {
        self.element
            .map_or(0, |element| self.length / element.width())
    }
}
