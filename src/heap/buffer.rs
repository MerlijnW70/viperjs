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
    /// Whether this is §25.2's `SharedArrayBuffer` rather than §25.1's `ArrayBuffer`.
    ///
    /// One flag rather than two types, because the bytes and every operation over them are the
    /// same. What differs is the *brand*: `ArrayBuffer.prototype.byteLength` requires an unshared
    /// buffer and `SharedArrayBuffer.prototype.byteLength` a shared one, so neither answers about
    /// the other — and `transfer` refuses a shared buffer outright, because §25.2 has no
    /// `[[ArrayBufferDetachKey]]` at all. A shared buffer can never be detached, which is the
    /// whole of what "shared" means to an engine with one agent.
    shared: bool,
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
            shared: false,
            bytes: Some(vec![0; length]),
        }
    }

    /// The same, as §25.2.2.1's `SharedArrayBuffer` — bytes that nothing can take away.
    #[must_use]
    pub fn new_shared(length: usize) -> Self {
        let mut made = Self::new(length);
        made.shared = true;
        made
    }

    /// Whether this is a `SharedArrayBuffer`.
    #[must_use]
    pub fn shared(&self) -> bool {
        self.shared
    }

    /// The bytes, or `None` once the buffer has been detached.
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

/// The ten numeric types §25.1.3.1's bytes can be read as.
///
/// Shared by §25.3's `DataView`, which asks for one per access, and by §23.2's TypedArrays, which
/// each have one for their whole length. That is the only difference between the two: a `DataView`
/// is a window with no type and a TypedArray is a window with one.
///
/// # Eight of them hold a Number and two hold a BigInt
///
/// §23.2.1 calls that a `[[ContentType]]`, and it is the one thing about a kind that a program
/// cannot coerce its way past: writing `1` to a `BigInt64Array` is a TypeError rather than a
/// conversion, because §6.1.6 gives Number and BigInt no arithmetic in common and an implicit
/// crossing between them would be exactly the silent precision loss the type exists to prevent.
/// [`Element::holds_big`] is that question, and every read and write below is decided by it.
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
    /// A signed 64-bit integer, read and written as a BigInt.
    BigInt64,
    /// An unsigned 64-bit integer, read and written as a BigInt.
    BigUint64,
    /// An IEEE single.
    Float32,
    /// An IEEE double.
    Float64,
}

/// §6.1.6's two numeric types, as whichever one a slot holds.
///
/// The value a read of an [`Element`] answers and a write of one is given, and it exists because
/// those two are no longer both Numbers. Not a [`Value`](crate::value::Value): a BigInt in the heap
/// is an identifier, and the bytes of a buffer can be read without one — which is what lets
/// [`Element::read`] stay a function of the bytes alone and leaves the allocation to whoever wanted
/// a JavaScript value out of it.
#[derive(Debug, Clone, PartialEq)]
pub enum Numeric {
    /// §6.1.6.1's Number, which eight of the ten kinds hold.
    Number(f64),
    /// §6.1.6.2's BigInt, which the two 64-bit integer kinds hold.
    BigInt(crate::bigint::BigInt),
}

impl Element {
    /// How many bytes one of these takes — §25.3.1.1's element size.
    #[must_use]
    pub fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 => 1,
            Self::Int16 | Self::Uint16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::BigInt64 | Self::BigUint64 | Self::Float64 => 8,
        }
    }

    /// §23.2.1's `[[ContentType]]` — whether a slot of this kind holds a BigInt rather than a Number.
    ///
    /// The question every write asks first, because it chooses the conversion: §7.1.13 `ToBigInt`
    /// for the two, §7.1.4 `ToNumber` for the eight. It is also what §23.2.3.24's `set` and
    /// §23.2.5.1.2's copy-construction compare between two arrays, since bytes can be copied between
    /// kinds of one content type and never between the two.
    #[must_use]
    pub fn holds_big(self) -> bool {
        matches!(self, Self::BigInt64 | Self::BigUint64)
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
            Self::BigInt64 => "BigInt64",
            Self::BigUint64 => "BigUint64",
            Self::Float32 => "Float32",
            Self::Float64 => "Float64",
        }
    }

    /// §25.3.1.3 `RawBytesToNumeric` — the bytes, already in reading order, as a numeric value.
    ///
    /// Fewer bytes than the kind is wide reads the rest as zero rather than refusing, because every
    /// caller has already checked the bounds and a second refusal here would be one nothing could
    /// reach.
    #[must_use]
    pub fn read(self, bytes: &[u8]) -> Numeric {
        let mut eight = [0_u8; 8];
        eight[..bytes.len()].copy_from_slice(bytes);
        let number = match self {
            Self::Int8 => f64::from(eight[0] as i8),
            Self::Uint8 => f64::from(eight[0]),
            Self::Int16 => f64::from(i16::from_le_bytes([eight[0], eight[1]])),
            Self::Uint16 => f64::from(u16::from_le_bytes([eight[0], eight[1]])),
            Self::Int32 => f64::from(i32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            Self::Uint32 => f64::from(u32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]])),
            // §25.3.1.3 step 5 — the same eight bytes are two different BigInts depending on
            // whether the top bit is a sign, and that is the whole of the difference between the
            // two kinds. Nothing else about them differs, not even the write.
            Self::BigInt64 | Self::BigUint64 => {
                let bits = u64::from_le_bytes(eight);
                return Numeric::BigInt(crate::bigint::BigInt::from_bits(
                    bits,
                    self == Self::BigInt64,
                ));
            }
            // §6.1.6.1's Number is a double, so a float32 widens on the way out. Every float32 is
            // exactly representable as a double, so nothing is lost — but the *value* is the
            // rounded one, which is why `v.setFloat32(0, 0.1); v.getFloat32(0)` is not `0.1`.
            Self::Float32 => {
                f64::from(f32::from_le_bytes([eight[0], eight[1], eight[2], eight[3]]))
            }
            Self::Float64 => f64::from_le_bytes(eight),
        };
        Numeric::Number(number)
    }

    /// §25.3.1.5 `NumericToRawBytes` for a Number — `None` if this kind holds a BigInt.
    ///
    /// The integer conversions are `ToIntN`/`ToUintN` (§7.1.7 and following), which **wrap** rather
    /// than clamp or throw: `setUint8(0, 256)` writes 0, and `setInt8(0, 200)` writes -56. That is
    /// modular arithmetic and not saturation, and it is what makes a TypedArray of bytes behave
    /// like memory rather than like a checked container.
    ///
    /// `None` rather than a wrap into the 64-bit slots, because a Number reaching a
    /// `BigInt64Array` is not a value to be truncated — it is a program that should already have
    /// been refused by §7.1.13, and writing it would turn a TypeError into a silent conversion.
    #[must_use]
    pub fn write(self, value: f64) -> Option<Vec<u8>> {
        Some(match self {
            Self::Int8 | Self::Uint8 => vec![wrap(value, 8) as u8],
            Self::Int16 | Self::Uint16 => (wrap(value, 16) as u16).to_le_bytes().to_vec(),
            Self::Int32 | Self::Uint32 => (wrap(value, 32) as u32).to_le_bytes().to_vec(),
            Self::Float32 => (value as f32).to_le_bytes().to_vec(),
            Self::Float64 => value.to_le_bytes().to_vec(),
            Self::BigInt64 | Self::BigUint64 => return None,
        })
    }

    /// §25.3.1.5 `NumericToRawBytes` for a BigInt — `None` if this kind holds a Number.
    ///
    /// §7.1.15 `ToBigInt64` and §7.1.16 `ToBigUint64`, which are one operation here: both take the
    /// low sixty-four bits and differ only in how a *read* interprets the top one. So there is no
    /// sign in this function, which is why `setBigInt64` and `setBigUint64` are the same native.
    #[must_use]
    pub fn write_big(self, value: &crate::bigint::BigInt) -> Option<Vec<u8>> {
        match self.holds_big() {
            true => Some(value.low_u64().to_le_bytes().to_vec()),
            false => None,
        }
    }

    /// §25.3.1.5 for whichever of the two a value is, with §7.1.11's clamping where it is asked for.
    ///
    /// `None` when the numeric and the kind disagree about their content type, which no caller can
    /// arrive at: the conversion that produced the value was itself chosen by this kind, and both
    /// §7.1.4 and §7.1.13 throw rather than answer the other type. Written as an absence rather
    /// than a truncation so that a caller that ever *did* skip the conversion writes nothing at all
    /// instead of writing the wrong sixty-four bits.
    #[must_use]
    pub fn write_numeric(self, value: &Numeric, clamped: bool) -> Option<Vec<u8>> {
        match value {
            Numeric::Number(number) => self.write(clamp_if(*number, clamped)),
            Numeric::BigInt(big) => self.write_big(big),
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

/// §7.1.11 `ToUint8Clamp`, applied only where a `Uint8ClampedArray` asks for it.
///
/// Saturating rather than wrapping, and rounding halves to **even** rather than away from zero.
/// Both are what pixel data wants: 300 is "as bright as it gets" rather than 44, and rounding half
/// to even keeps a long run of averages from drifting upwards.
///
/// Beside the wrapping this module's integer writes use, which is its counterpart: the two are §7.1
/// asking what a Number becomes at a fixed width. Clamping is the *only* thing that separates
/// `Uint8ClampedArray` from `Uint8Array` — every read of their bytes is identical, so the whole of
/// the difference between two of the eleven kinds is this function.
#[must_use]
pub fn clamp_if(value: f64, clamped: bool) -> f64 {
    if !clamped {
        return value;
    }
    // No case for `NaN`: `f64::clamp` answers `NaN` for one, and every arithmetic below carries it
    // through to the `write` that follows, where §7.1.9's cast turns it into 0 — which is the
    // answer §7.1.11 step 1 asks for. Written out as a case it was a branch nothing could tell
    // from its absence.
    let bounded = value.clamp(0.0, 255.0);
    let floor = bounded.floor();
    match bounded - floor {
        half if half > 0.5 => floor + 1.0,
        half if half < 0.5 => floor,
        // Exactly a half — to *even*, which `f64::round` does not do.
        _ if floor % 2.0 == 0.0 => floor,
        _ => floor + 1.0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bigint::BigInt;

    /// Every kind and the width §25.3.1.1's table gives it.
    ///
    /// Ten where [`KINDS`](crate::heap::KINDS) has eleven, because `Uint8ClampedArray` is a
    /// `Uint8` that saturates rather than a kind of its own.
    const WIDTHS: [(Element, usize); 10] = [
        (Element::Int8, 1),
        (Element::Uint8, 1),
        (Element::Int16, 2),
        (Element::Uint16, 2),
        (Element::Int32, 4),
        (Element::Uint32, 4),
        (Element::BigInt64, 8),
        (Element::BigUint64, 8),
        (Element::Float32, 4),
        (Element::Float64, 8),
    ];

    #[test]
    fn a_kind_writes_only_the_numeric_type_it_holds() {
        // The content-type line, at the one place it is a *type* question rather than a
        // conversion. Every write above this has already run §7.1.4 or §7.1.13 — whichever the
        // kind asked for — so a mismatched pair reaching here is a caller that skipped the
        // conversion. Answering an absence rather than truncating is what makes that a write
        // that does not happen instead of the wrong sixty-four bits.
        for (element, width) in WIDTHS {
            let number = element.write(1.0);
            let big = element.write_big(&BigInt::from_u64(1));
            match element.holds_big() {
                true => {
                    assert_eq!(number, None, "{} took a Number", element.name());
                    assert_eq!(
                        big.map(|bytes| bytes.len()),
                        Some(width),
                        "{} refused a BigInt",
                        element.name()
                    );
                }
                false => {
                    assert_eq!(big, None, "{} took a BigInt", element.name());
                    assert_eq!(
                        number.map(|bytes| bytes.len()),
                        Some(width),
                        "{} refused a Number",
                        element.name()
                    );
                }
            }
        }
    }

    #[test]
    fn write_numeric_picks_the_half_that_matches_and_has_none_for_the_other() {
        // The dispatch [`Heap::set_element`] and the `DataView` setters both go through, so a
        // mistake in it is a mistake in every write in the engine. Checked in both directions:
        // the pair that agrees produces bytes and the pair that does not produces nothing.
        let one = Numeric::Number(1.0);
        let big_one = Numeric::BigInt(BigInt::from_u64(1));
        assert_eq!(
            Element::Int8.write_numeric(&one, false),
            Some(vec![1]),
            "a Number into a Number kind"
        );
        assert_eq!(
            Element::BigInt64.write_numeric(&big_one, false),
            Some(vec![1, 0, 0, 0, 0, 0, 0, 0]),
            "a BigInt into a BigInt kind"
        );
        assert_eq!(
            Element::BigInt64.write_numeric(&one, false),
            None,
            "a Number into a BigInt kind"
        );
        assert_eq!(
            Element::Int8.write_numeric(&big_one, false),
            None,
            "a BigInt into a Number kind"
        );
        // …and the clamping flag still reaches the Number half through it, which is the one thing
        // `write_numeric` does beyond choosing: §7.1.11 saturates where §7.1.9 wraps.
        assert_eq!(
            Element::Uint8.write_numeric(&Numeric::Number(300.0), true),
            Some(vec![255]),
            "clamped"
        );
        assert_eq!(
            Element::Uint8.write_numeric(&Numeric::Number(300.0), false),
            Some(vec![44]),
            "unclamped"
        );
    }

    #[test]
    fn the_two_bigint_kinds_are_the_same_eight_bytes_read_two_ways() {
        // §25.3.1.3 step 5 — the whole of the difference between `BigInt64` and `BigUint64` is
        // whether the top bit is a sign. Written through one and read through both says that in a
        // way two separate round trips would not.
        let bytes = Element::BigInt64
            .write_big(&BigInt::from_u64(1).negate())
            .expect("a BigInt kind takes a BigInt"); // the test is about the bytes
        assert_eq!(bytes, vec![0xFF; 8]);
        assert_eq!(
            Element::BigInt64.read(&bytes),
            Numeric::BigInt(BigInt::from_u64(1).negate()),
            "read as signed"
        );
        assert_eq!(
            Element::BigUint64.read(&bytes),
            Numeric::BigInt(BigInt::from_u64(u64::MAX)),
            "read as unsigned"
        );
        // And the *write* consults no sign at all, which is why one native serves both setters.
        assert_eq!(
            Element::BigUint64.write_big(&BigInt::from_u64(1).negate()),
            Some(bytes),
            "the sign is a reader's question"
        );
    }

    #[test]
    fn a_kinds_width_is_what_its_bytes_take_and_its_name_is_its_accessors() {
        // The table §25.3.1.1 and §25.3.4 share. A width that disagreed with what `write`
        // produces would put every element of an array at the wrong offset, and a name that
        // disagreed would build `DataView.prototype` with a method under the wrong spelling.
        for (element, width) in WIDTHS {
            assert_eq!(element.width(), width, "{}", element.name());
            let bytes = match element.holds_big() {
                true => element.write_big(&BigInt::zero()),
                false => element.write(0.0),
            };
            assert_eq!(
                bytes.map(|bytes| bytes.len()),
                Some(width),
                "{} wrote its width",
                element.name()
            );
        }
        assert_eq!(Element::BigInt64.name(), "BigInt64");
        assert_eq!(Element::BigUint64.name(), "BigUint64");
    }
}
