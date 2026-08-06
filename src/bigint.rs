//! §6.1.6.2's BigInt — an integer with no width, and the arithmetic §6.1.6.2.1 to §6.1.6.2.22 asks of it.
//!
//! A sign and a magnitude, where the magnitude is base 2^32 with the least significant limb first.
//! Two invariants hold everywhere and every operation restores them: there is **no trailing zero
//! limb**, and **zero is not negative**. Together they make equality a comparison of two fields and
//! nothing more, which is what §6.1.6.2.13's `BigInt::equal` wants.
//!
//! # Why not two's complement
//!
//! Because a BigInt has no width, and two's complement is a statement about one. `-1n` in two's
//! complement is an infinite run of ones, which a `Vec` cannot hold. Sign-and-magnitude holds every
//! value in the space a program can name and pays for it in exactly one place: §6.1.6.2.18 to
//! §6.1.6.2.20's bitwise operators, which *are* defined on two's complement and are written below
//! as the identities that relate the two.
//!
//! # Why base 2^32 and not 2^64
//!
//! A multiply of two limbs has to fit in something. `u32 * u32` fits in a `u64` and needs nothing
//! from the platform; `u64 * u64` needs a 128-bit product, and while Rust has `u128` the schoolbook
//! algorithms below read the same either way. Twice the limbs is twice the loop iterations on
//! numbers that programs almost never make large — a `BigInt` in real code is a database identifier
//! or a nanosecond timestamp, not a cryptographic modulus.

use std::cmp::Ordering;

/// An arbitrary-precision integer — §6.1.6.2's BigInt value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BigInt {
    /// Whether this is less than zero. Never true when the magnitude is empty.
    negative: bool,
    /// Base 2^32, least significant limb first, with no trailing zero. Empty is zero.
    magnitude: Vec<u32>,
}

/// Why an operation could not answer — each is a specific abrupt completion in §6.1.6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// §6.1.6.2.5 `BigInt::divide` step 1 and §6.1.6.2.6 step 1 — a RangeError, not a NaN.
    ///
    /// The one place BigInt and Number part company loudest: `1 / 0` is `Infinity` and `1n / 0n`
    /// throws, because there is no BigInt infinity for it to be.
    DividedByZero,
    /// §6.1.6.2.3 `BigInt::exponentiate` step 1 — a negative exponent is a RangeError.
    ///
    /// `2n ** -1n` would be one half, and a BigInt is an integer. Refusing is the only answer that
    /// is not a lie about the type.
    NegativeExponent,
    /// §6.1.6.2.11 `BigInt::unsignedRightShift` — always a TypeError.
    ///
    /// `>>>` fills from the left with zeros, which needs a width; a BigInt has none. The
    /// specification does not define it rather than defining it as `>>`.
    NoUnsignedShift,
    /// A result too large to hold — ViperJS's limit, not the language's.
    ///
    /// §6.1.6.2 puts no bound on a BigInt, and no implementation can honour that. `2n ** (2n **
    /// 40n)` is a number nothing can write down, and asking for it should be a refusal rather than
    /// an allocation that never returns. The ceiling is a gigabyte of magnitude.
    TooLarge,
}

/// The most limbs a BigInt may have — ViperJS's ceiling on §6.1.6.2's unbounded integer.
///
/// 2^20 limbs is four megabytes of magnitude — a thirty-three-million-bit integer, or ten million
/// decimal digits. Past what any program means to compute, and *inside* DR-0013's sixty-four
/// mebibyte heap besides, so a program that reaches it was going to run out of heap anyway.
///
/// Chosen small enough that a test can stand at the boundary. A ceiling nobody can reach is a
/// ceiling nobody has checked, and the two sides of it are one comparison apart.
pub const MAX_LIMBS: usize = 1 << 20;

impl BigInt {
    /// Zero.
    pub fn zero() -> Self {
        // Through `from_u64` rather than an empty magnitude and a sign of its own: the sign of an
        // empty magnitude is dropped, so writing one here was a value nothing could read.
        Self::from_u64(0)
    }

    /// The BigInt this `u64` names.
    pub fn from_u64(value: u64) -> Self {
        Self::from_parts(vec![value as u32, (value >> 32) as u32], false)
    }

    /// Whether this is zero — §6.1.6.2's only value that is neither positive nor negative.
    pub fn is_zero(&self) -> bool {
        self.magnitude.is_empty()
    }

    /// Whether this is less than zero.
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// A value from a magnitude and a sign, restoring both invariants.
    ///
    /// The one way to build one, so that "zero is not negative" is decided in a single place. It
    /// was written out at each site for a while, as `negative: false` followed by a sign applied
    /// afterwards — and the `false` was then a value nothing could read, because the sign always
    /// overwrote it.
    fn from_parts(mut magnitude: Vec<u32>, negative: bool) -> Self {
        trim(&mut magnitude);
        Self {
            negative: negative && !magnitude.is_empty(),
            magnitude,
        }
    }

    /// The same magnitude with this sign, keeping zero unsigned.
    fn with_sign(self, negative: bool) -> Self {
        Self::from_parts(self.magnitude, negative)
    }

    /// §6.1.6.2.1 `BigInt::unaryMinus` — the same magnitude, the other sign.
    ///
    /// Zero negates to zero, which is not a special case here but a consequence of the invariant
    /// at the top of this file: an empty magnitude is never signed.
    pub fn negate(&self) -> Self {
        self.clone().with_sign(!self.negative)
    }

    /// The low 64 bits of this value's two's complement — what a fixed-width slot receives.
    ///
    /// §25.3.1.2 writes a BigInt into eight bytes, and §21.2.2.2's `asUintN(64, …)` is the same
    /// arithmetic: everything modulo 2^64. A negative value is the bits its complement has, which
    /// is why this reads the magnitude and then negates rather than truncating a signed number
    /// that may not fit in one.
    pub fn low_u64(&self) -> u64 {
        let low = u64::from(self.magnitude.first().copied().unwrap_or(0));
        let high = u64::from(self.magnitude.get(1).copied().unwrap_or(0));
        let bits = low | (high << 32);
        match self.negative {
            true => bits.wrapping_neg(),
            false => bits,
        }
    }

    /// The BigInt these 64 bits name, read as signed or unsigned.
    ///
    /// The other direction, and the one place the two `BigInt64` element kinds differ: the same
    /// eight bytes are `-1n` read as signed and `18446744073709551615n` read as unsigned.
    pub fn from_bits(bits: u64, signed: bool) -> Self {
        match signed && bits & 0x8000_0000_0000_0000 != 0 {
            true => Self::from_u64(bits.wrapping_neg()).negate(),
            false => Self::from_u64(bits),
        }
    }

    /// This value negated when `negative`, and unchanged otherwise.
    ///
    /// For a caller that has read a sign and some digits separately, which is what every text
    /// form of a BigInt is.
    pub fn negate_if(self, negative: bool) -> Self {
        match negative {
            true => self.negate(),
            false => self,
        }
    }

    /// The magnitude, as a positive BigInt — `|x|`.
    pub fn magnitude_of(&self) -> Self {
        self.clone().with_sign(false)
    }

    /// §6.1.6.2.12 `BigInt::lessThan`, as the full ordering the callers of it want.
    ///
    /// A negative is below every non-negative, and among two of the same sign the magnitudes decide
    /// — reversed for two negatives, since a bigger magnitude is a smaller number.
    pub fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => compare_magnitude(&self.magnitude, &other.magnitude),
            (true, true) => compare_magnitude(&other.magnitude, &self.magnitude),
        }
    }

    /// §6.1.6.2.7 `BigInt::add`.
    pub fn add(&self, other: &Self) -> Result<Self, Error> {
        // Like signs add the magnitudes and keep the sign; unlike signs are a subtraction, and
        // which way round it goes is decided by which magnitude is larger.
        if self.negative == other.negative {
            let magnitude = add_magnitude(&self.magnitude, &other.magnitude)?;
            return Ok(Self::from_parts(magnitude, self.negative));
        }
        Ok(match compare_magnitude(&self.magnitude, &other.magnitude) {
            Ordering::Less => Self::from_parts(
                subtract_magnitude(&other.magnitude, &self.magnitude),
                other.negative,
            ),
            _ => Self::from_parts(
                subtract_magnitude(&self.magnitude, &other.magnitude),
                self.negative,
            ),
        })
    }

    /// §6.1.6.2.8 `BigInt::subtract` — `x + (-y)`, which is what the clause is.
    pub fn subtract(&self, other: &Self) -> Result<Self, Error> {
        self.add(&other.negate())
    }

    /// §6.1.6.2.4 `BigInt::multiply`.
    pub fn multiply(&self, other: &Self) -> Result<Self, Error> {
        // No shortcut for a zero operand: `multiply_magnitude` walks an empty magnitude, produces
        // limbs of zeros and trims them away, which is the same answer by the same route.
        let magnitude = multiply_magnitude(&self.magnitude, &other.magnitude)?;
        Ok(Self::from_parts(magnitude, self.negative != other.negative))
    }

    /// §6.1.6.2.5 `BigInt::divide` — truncated towards zero, as the clause says.
    ///
    /// `-7n / 2n` is `-3n` and not `-4n`: the quotient is the *integer part*, which is a different
    /// thing from the floor for a negative result. `%` below is defined to agree with it.
    pub fn divide(&self, other: &Self) -> Result<Self, Error> {
        Ok(self.divide_and_remainder(other)?.0)
    }

    /// §6.1.6.2.6 `BigInt::remainder` — the sign of the **dividend**, as the clause says.
    ///
    /// `-7n % 2n` is `-1n`, not `1n`. That is not the mathematical modulo and it is what makes
    /// `(a / b) * b + (a % b)` equal `a`, which is the identity the pair is defined by.
    pub fn remainder(&self, other: &Self) -> Result<Self, Error> {
        Ok(self.divide_and_remainder(other)?.1)
    }

    /// Both halves of a division, which the algorithm produces together.
    pub fn divide_and_remainder(&self, other: &Self) -> Result<(Self, Self), Error> {
        if other.is_zero() {
            return Err(Error::DividedByZero);
        }
        let (quotient, remainder) = divide_magnitude(&self.magnitude, &other.magnitude)?;
        Ok((
            Self::from_parts(quotient, self.negative != other.negative),
            Self::from_parts(remainder, self.negative),
        ))
    }

    /// §6.1.6.2.3 `BigInt::exponentiate`.
    ///
    /// Square-and-multiply, which is the difference between `2n ** 1000n` being instant and being a
    /// thousand multiplications of a growing number. The exponent is read as a `u64` because an
    /// exponent that does not fit in one names a result with more bits than there are atoms; that
    /// case is [`Error::TooLarge`] rather than an attempt.
    pub fn exponentiate(&self, exponent: &Self) -> Result<Self, Error> {
        if exponent.negative {
            return Err(Error::NegativeExponent);
        }
        let Some(power) = exponent.to_u64() else {
            return Err(Error::TooLarge);
        };
        // Left to right over the exponent's bits, squaring the running result rather than the
        // base. The other direction needs a guard against squaring the base one last time after
        // the loop has finished with it — a step whose answer nothing reads, which is a branch no
        // test can distinguish.
        let mut result = Self::from_u64(1);
        for bit in (0..(u64::BITS - power.leading_zeros())).rev() {
            result = result.multiply(&result)?;
            if power >> bit & 1 == 1 {
                result = result.multiply(self)?;
            }
        }
        Ok(result)
    }

    /// §6.1.6.2.9 `BigInt::leftShift`, which shifts the other way for a negative count.
    ///
    /// `x << -n` is `x >> n` — the clause is written as one operation taking a signed count, and
    /// this is that.
    pub fn shift_left(&self, places: &Self) -> Result<Self, Error> {
        if places.negative {
            return self.shift_right(&places.negate());
        }
        let Some(places) = places.to_u64() else {
            return Err(Error::TooLarge);
        };
        let magnitude = shift_magnitude_left(&self.magnitude, places)?;
        Ok(Self::from_parts(magnitude, self.negative))
    }

    /// §6.1.6.2.10 `BigInt::signedRightShift`, which is an *arithmetic* shift.
    ///
    /// The sign is kept, and a negative number rounds **towards negative infinity** rather than
    /// towards zero: `-1n >> 1n` is `-1n`, where `-1n / 2n` is `0n`. That is what makes a right
    /// shift a division by a power of two in two's complement and not in sign-and-magnitude — so
    /// the correction below is the whole of the difference.
    pub fn shift_right(&self, places: &Self) -> Result<Self, Error> {
        if places.negative {
            return self.shift_left(&places.negate());
        }
        let Some(places) = places.to_u64() else {
            // Shifting a finite number right by more bits than it has leaves 0, or -1 for a
            // negative — the sign bit, repeated for ever.
            return Ok(match self.negative {
                true => Self::from_u64(1).negate(),
                false => Self::zero(),
            });
        };
        let shifted = shift_magnitude_right(&self.magnitude, places);
        let result = Self::from_parts(shifted, self.negative);
        // A negative that lost any set bit on the way out has rounded the wrong way for an
        // arithmetic shift, and one more towards negative infinity is the correction.
        match self.negative && !dropped_bits_were_zero(&self.magnitude, places) {
            true => result.subtract(&Self::from_u64(1)),
            false => Ok(result),
        }
    }

    /// §6.1.6.2.2 `BigInt::bitwiseNOT` — `-(x + 1)`, which is what two's complement makes it.
    ///
    /// Written as the identity rather than as a walk over the limbs, because the identity is exact
    /// at every width and a walk is a statement about one.
    pub fn not(&self) -> Result<Self, Error> {
        self.add(&Self::from_u64(1)).map(|sum| sum.negate())
    }

    /// §6.1.6.2.20 `BigInt::bitwiseAND`.
    pub fn and(&self, other: &Self) -> Result<Self, Error> {
        self.bitwise(other, |a, b| a & b)
    }

    /// §6.1.6.2.19 `BigInt::bitwiseXOR`.
    pub fn xor(&self, other: &Self) -> Result<Self, Error> {
        self.bitwise(other, |a, b| a ^ b)
    }

    /// §6.1.6.2.18 `BigInt::bitwiseOR`.
    pub fn or(&self, other: &Self) -> Result<Self, Error> {
        self.bitwise(other, |a, b| a | b)
    }

    /// The three bitwise operators, over two's complement, at a width wide enough to be all widths.
    ///
    /// §6.1.6.2.17 `BigIntBitwiseOp` is defined on the infinite two's-complement expansions, where
    /// a negative number is an infinite run of leading ones. An infinite run is not something to
    /// hold, but it *is* something to know: past the last limb of a negative operand every bit is
    /// one, so a width one limb beyond the longer operand computes the same answer as any wider
    /// one, and its top limb says which.
    fn bitwise(&self, other: &Self, combine: impl Fn(u32, u32) -> u32) -> Result<Self, Error> {
        let width = self.magnitude.len().max(other.magnitude.len()) + 1;
        within_ceiling(width)?;
        let left = self.to_twos_complement(width);
        let right = other.to_twos_complement(width);
        let combined: Vec<u32> = left
            .iter()
            .zip(right.iter())
            .map(|(a, b)| combine(*a, *b))
            .collect();
        Ok(Self::from_twos_complement(combined))
    }

    /// This value as `width` limbs of two's complement — see [`BigInt::bitwise`].
    fn to_twos_complement(&self, width: usize) -> Vec<u32> {
        let mut limbs = vec![0u32; width];
        limbs[..self.magnitude.len()].copy_from_slice(&self.magnitude);
        if !self.negative {
            return limbs;
        }
        // Negate: invert every limb and add one, which is the definition and not an optimisation of
        // it. The carry cannot escape `width`, because `width` is a limb wider than the magnitude.
        let mut carry = 1u64;
        for limb in &mut limbs {
            let sum = u64::from(!*limb) + carry;
            *limb = sum as u32;
            carry = sum >> 32;
        }
        limbs
    }

    /// The reverse, reading the top limb as the sign.
    fn from_twos_complement(mut limbs: Vec<u32>) -> Self {
        // The top limb is all ones for a negative and all zeros for a non-negative, `width` having
        // been chosen a limb wider than either operand needed.
        let negative = limbs.last().is_some_and(|top| *top & 0x8000_0000 != 0);
        if negative {
            let mut carry = 1u64;
            for limb in &mut limbs {
                let sum = u64::from(!*limb) + carry;
                *limb = sum as u32;
                carry = sum >> 32;
            }
        }
        trim(&mut limbs);
        Self::from_parts(limbs, negative)
    }

    /// The BigInt this `f64` names *exactly*, or `None` if it does not name one.
    ///
    /// `None` for a NaN, an infinity and anything with a fractional part — §7.2.15 step 5 asks
    /// whether two values are the same point on the number line, and a Number that is not an
    /// integer is not any BigInt.
    ///
    /// Exact, by taking the mantissa and the exponent apart rather than going through a decimal
    /// string. That matters at the one place anybody notices: `2n ** 53n + 1n` and `2 ** 53` are
    /// different numbers, and a conversion that went through `f64` in either direction would say
    /// they are the same.
    pub fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() || value.fract() != 0.0 {
            return None;
        }
        let bits = value.to_bits();
        let negative = bits >> 63 == 1;
        let raw_exponent = ((bits >> 52) & 0x7FF) as i64;
        let fraction = bits & 0x000F_FFFF_FFFF_FFFF;
        // A subnormal has no implicit leading one and an exponent of -1074; a normal has both. The
        // only subnormal that is an integer is zero, which the shift below then answers for.
        let (mantissa, exponent) = match raw_exponent {
            0 => (fraction, -1074),
            _ => (fraction | 0x0010_0000_0000_0000, raw_exponent - 1075),
        };
        // One shift with a *signed* count rather than a branch on its sign: `shift_left` already
        // turns a negative count round, and a branch here would have had two arms answering the
        // same for an exponent of zero — the one value at their boundary.
        //
        // A negative exponent with no fractional part means the mantissa's low bits are zero, so
        // shifting the other way discards nothing; `value.fract()` above is what guarantees it.
        let places = Self::from_u64(exponent.unsigned_abs()).negate_if(exponent.is_negative());
        // The only error a shift has is `TooLarge`, and `None` is already this function's answer
        // for a Number with no BigInt — so the two agree. A finite double's exponent cannot reach
        // `MAX_LIMBS` anyway, which makes this the contract rather than a case.
        let shifted = Self::from_u64(mantissa).shift_left(&places).ok()?; // no BigInt for it
        Some(shifted.with_sign(negative))
    }

    /// The Number nearest this value — `𝔽(ℝ(x))`, which §21.1.1.1 is the only caller of.
    ///
    /// **Not** a conversion the language performs on its own. §7.1.4 `ToNumber` refuses a BigInt
    /// outright, which is what keeps `1n + 1` a TypeError; the one operation that crosses is
    /// `Number(x)`, and it says so in a step of its own rather than by calling `ToNumber`. So this
    /// is deliberately not `impl From<BigInt> for f64` — a conversion that is available implicitly
    /// is one that will be reached implicitly.
    ///
    /// Rounded **to nearest, ties to even**, which is what `𝔽` means for a mathematical value that
    /// is not exactly a double. `Number(2n ** 53n + 1n)` is 9007199254740992, one less than the
    /// integer asked for, and that is right: the answer is the nearest Number, and there is none in
    /// between. A value past the largest finite double is `Infinity`.
    ///
    /// Written from the bits rather than by accumulating limbs into an `f64`. Accumulating rounds
    /// at every step, so a value needing more than 53 bits would be rounded twice and land a unit
    /// out — the same double-rounding argument [`BigInt::from_f64`] makes in the other direction.
    pub fn to_f64(&self) -> f64 {
        let bits = self.bit_length();
        // A double keeps 53 significant bits. Anything shorter is exact and needs no rounding at
        // all — and cannot be built by the path below, whose shift would be negative.
        //
        // Zero comes through here rather than by a test of its own: it has no bits, so it is
        // shorter, and §6.1.6.2 gives it no negative form for the sign below to find. A branch for
        // it was written first and answered `0.0` exactly as this does, which made it a line no
        // input could distinguish.
        if bits <= 53 {
            // Through `bits_above` rather than [`BigInt::low_u64`], which applies the sign as a
            // two's complement — `-1n` would come back as 2^64 - 1 and convert to 1.8e19.
            let magnitude = self.bits_above(0) as f64;
            return match self.negative {
                true => -magnitude,
                false => magnitude,
            };
        }
        let discarded = bits - 53;
        // The top 53 bits, and the two facts §5.2's rounding needs about everything below them:
        // whether the next bit down is set, and whether anything below *that* is.
        let mut mantissa = self.bits_above(discarded);
        let halfway = self.bit(discarded - 1);
        let below = self.any_bit_below(discarded - 1);
        let mut exponent = i64::from(discarded);
        // Ties to even: round up when past halfway, and exactly at halfway only when rounding up
        // makes the last bit zero. Dropping the `below` term would round `2n ** 53n + 1n` up, and
        // dropping the parity term would round `2n ** 53n + 2n` up — the two are separate cases and
        // each has a test.
        if halfway && (below || mantissa & 1 == 1) {
            mantissa += 1;
            // Carrying out of 53 bits — the rounded value is a power of two one place wider. Only
            // the exponent moves: the mantissa is now exactly 2^53, whose low fifty-two bits are
            // zero, and those are the only ones the format below stores. Halving it as well was
            // written first and is a line no input can distinguish, because 2^53 and 2^52 have the
            // same stored fraction and the exponent is what tells them apart.
            if mantissa == 1 << 53 {
                exponent += 1;
            }
        }
        // The mantissa's leading bit is implicit in the format, so what is stored is the low 52 and
        // the unbiased exponent is 52 more than the shift.
        let biased = exponent + 52 + 1023;
        if biased >= 0x7FF {
            // Past the largest finite double, which is what §5.2's `𝔽` answers with here.
            return match self.negative {
                true => f64::NEG_INFINITY,
                false => f64::INFINITY,
            };
        }
        let sign = u64::from(self.negative) << 63;
        // `biased` is in 1..0x7FF and `mantissa` has exactly 53 bits, so neither field can overflow
        // its own — which is what makes this an assembly of a double rather than an arithmetic on
        // one, and why no rounding happens here.
        let assembled = sign | ((biased as u64) << 52) | (mantissa & 0x000F_FFFF_FFFF_FFFF);
        f64::from_bits(assembled)
    }

    /// How many bits the magnitude needs — zero for zero.
    fn bit_length(&self) -> u32 {
        match self.magnitude.last() {
            None => 0,
            // The limbs below the top one are full, and the top one contributes what it uses.
            Some(top) => {
                let below = u32::try_from(self.magnitude.len() - 1).unwrap_or(u32::MAX);
                below.saturating_mul(32) + (32 - top.leading_zeros())
            }
        }
    }

    /// Whether the bit at `at` is set, counting from the least significant.
    fn bit(&self, at: u32) -> bool {
        let limb = (at / 32) as usize;
        self.magnitude
            .get(limb)
            .is_some_and(|limb| limb >> (at % 32) & 1 == 1)
    }

    /// Whether any bit strictly below `at` is set — §5.2's sticky bit.
    fn any_bit_below(&self, at: u32) -> bool {
        let whole = (at / 32) as usize;
        if self.magnitude.iter().take(whole).any(|limb| *limb != 0) {
            return true;
        }
        // The partial limb, masked to the bits below `at`. A shift of 32 is undefined, which is
        // why the remainder rather than `at` itself decides the mask.
        let used = at % 32;
        self.magnitude
            .get(whole)
            .is_some_and(|limb| limb & ((1u32 << used) - 1) != 0)
    }

    /// The value of the bits from `at` upwards, which the caller has bounded to 53 of them.
    fn bits_above(&self, at: u32) -> u64 {
        let limb = (at / 32) as usize;
        let offset = at % 32;
        // Three limbs, because 53 bits starting anywhere inside a limb can reach into a third —
        // and therefore a `u128`, since three limbs are ninety-six bits and a `u64` would drop the
        // most significant of them, which is the one the answer is mostly made of.
        let mut value: u128 = 0;
        for index in (limb..limb + 3).rev() {
            value = (value << 32) | u128::from(self.magnitude.get(index).copied().unwrap_or(0));
        }
        // The low limb of `value` is the one holding bit `at`, so the offset finishes it. Masking
        // to 53 bits needs the caller's guarantee that `at` is `bit_length - 53`; anything above is
        // zero anyway, and the mask says which bits this promised to answer with.
        u64::try_from((value >> offset) & ((1 << 53) - 1)).unwrap_or(u64::MAX) // masked to 53 bits
    }

    /// §12.9.3's `BigIntLiteral`, and §7.1.14's `StringToBigInt` — digits in a radix, read.
    ///
    /// The digits are already known to be digits: the lexer read them and §7.1.14's caller has
    /// checked the string. Anything that is not one answers `None` rather than being skipped, so a
    /// caller cannot hand this `"1 2"` and get `12n`.
    ///
    /// Multiply-and-add, one digit at a time. A base conversion can be done in `n log n` and the
    /// difference shows up at tens of thousands of digits — which a literal in a source file is
    /// not, and `BigInt("…")` on such a string is a program that has other problems.
    pub fn from_digits(digits: &str, radix: u32) -> Option<Self> {
        let base = Self::from_u64(u64::from(radix));
        let mut value = Self::zero();
        for digit in digits.chars() {
            let read = digit.to_digit(radix)?;
            // Both errors are `TooLarge` and nothing else, and `None` is already what this answers
            // for digits it cannot make a BigInt of — so more of them than `MAX_LIMBS` holds is
            // one such reason rather than a different kind of failure.
            value = value.multiply(&base).ok()?; // too many digits is "not a BigInt"
            value = value.add(&Self::from_u64(u64::from(read))).ok()?; // same
        }
        Some(value)
    }

    /// §6.1.6.2.22 `BigInt::toString` — the digits, with a `-` in front when it is negative.
    ///
    /// Repeated division by the radix, least significant digit first. No `n` suffix: §6.1.6.2.22
    /// does not put one there, and `String(1n)` is `"1"` — the suffix is syntax and not part of
    /// the value.
    /// A refusal is carried rather than spelled. The digits are produced by dividing, and a
    /// division this engine cannot normalise is a `RangeError` — where answering `"0"` for a
    /// number with ten million digits is a wrong answer that reads like a right one, which is what
    /// this used to do (GHSA-6976-qm5m-7mcj).
    pub fn to_digits(&self, radix: u32) -> Result<String, Error> {
        if self.is_zero() {
            return Ok("0".to_string());
        }
        let mut digits = Vec::new();
        let mut left = self.magnitude.clone();
        let divisor = [radix];
        while !left.is_empty() {
            let (quotient, remainder) = divide_magnitude(&left, &divisor)?;
            let digit = remainder.first().copied().unwrap_or(0);
            // `from_digit` answers `None` only above the radix, and a remainder is always below it.
            digits.push(char::from_digit(digit, radix).unwrap_or('0'));
            left = quotient;
        }
        if self.negative {
            digits.push('-');
        }
        Ok(digits.iter().rev().collect())
    }

    /// This value as a `u64`, or `None` if it does not fit — the sign is ignored.
    fn to_u64(&self) -> Option<u64> {
        match self.magnitude.len() {
            0 => Some(0),
            1 => Some(u64::from(self.magnitude[0])),
            2 => Some(u64::from(self.magnitude[0]) | (u64::from(self.magnitude[1]) << 32)),
            _ => None,
        }
    }
}

/// Whether a result of this many limbs is one this engine will build — see [`MAX_LIMBS`].
///
/// One comparison rather than the same one at each of the three places that grow a magnitude. Three
/// copies is three chances to write the edge the wrong way round, and only one of the three is
/// cheap enough for a test to stand at: an addition reaches the ceiling with one allocation where a
/// multiplication would need two operands of half of it and the time to multiply them.
fn within_ceiling(width: usize) -> Result<(), Error> {
    match width > MAX_LIMBS {
        true => Err(Error::TooLarge),
        false => Ok(()),
    }
}

/// Drop the trailing zero limbs, which is what keeps the representation unique.
fn trim(magnitude: &mut Vec<u32>) {
    while magnitude.last() == Some(&0) {
        magnitude.pop();
    }
}

/// Which of two magnitudes is larger — longer wins, then the most significant limb that differs.
fn compare_magnitude(left: &[u32], right: &[u32]) -> Ordering {
    match left.len().cmp(&right.len()) {
        Ordering::Equal => left.iter().rev().cmp(right.iter().rev()),
        other => other,
    }
}

/// `left + right`, on magnitudes.
fn add_magnitude(left: &[u32], right: &[u32]) -> Result<Vec<u32>, Error> {
    let width = left.len().max(right.len()) + 1;
    within_ceiling(width)?;
    let mut sum = Vec::new();
    let mut carry = 0u64;
    for at in 0..width {
        let total = carry
            + u64::from(left.get(at).copied().unwrap_or(0))
            + u64::from(right.get(at).copied().unwrap_or(0));
        sum.push(total as u32);
        carry = total >> 32;
    }
    trim(&mut sum);
    Ok(sum)
}

/// `left - right`, on magnitudes, where the caller has established that `left >= right`.
fn subtract_magnitude(left: &[u32], right: &[u32]) -> Vec<u32> {
    let mut difference = Vec::new();
    let mut borrow = 0i64;
    for (at, limb) in left.iter().enumerate() {
        let total = i64::from(*limb) - i64::from(right.get(at).copied().unwrap_or(0)) - borrow;
        // `left >= right` is the caller's promise, so the borrow always comes back out of a later
        // limb — the running value can be negative here and the whole cannot.
        let (limb, next) = match total < 0 {
            true => (total as u32, 1),
            false => (total as u32, 0),
        };
        difference.push(limb);
        borrow = next;
    }
    trim(&mut difference);
    difference
}

/// `left * right`, schoolbook — one pass per limb of the right operand.
fn multiply_magnitude(left: &[u32], right: &[u32]) -> Result<Vec<u32>, Error> {
    let width = left.len() + right.len();
    within_ceiling(width)?;
    let mut product = vec![0u32; width];
    for (at, factor) in right.iter().enumerate() {
        let mut carry = 0u64;
        for (offset, limb) in left.iter().enumerate() {
            let total =
                u64::from(*limb) * u64::from(*factor) + u64::from(product[at + offset]) + carry;
            product[at + offset] = total as u32;
            carry = total >> 32;
        }
        product[at + left.len()] = carry as u32;
    }
    trim(&mut product);
    Ok(product)
}

/// `left / right` and `left % right`, on magnitudes — one algorithm, whatever the sizes.
///
/// A dividend smaller than the divisor and a divisor of a single limb both used to have a shortcut
/// of their own. Neither changed an answer: the general path produces a zero quotient for the
/// first and reads one limb where it would have read two for the second, and both shortcuts were
/// branches nothing could tell from their absence.
/// One digit of the quotient per position, most significant first, exactly as long division is
/// taught. The digit is *estimated* from the top limbs and then **corrected by trying it**: the
/// product is compared against what is left and the estimate comes down until it fits.
///
/// # Why not Knuth's algorithm D as written
///
/// Algorithm D estimates more cleverly — normalising the divisor so its top bit is set bounds the
/// error at one — and then subtracts optimistically and *adds back* on the rare occasion it went
/// too far. That add-back is correct and it is unreachable by any input a test can construct: it
/// runs about once in two billion divisions of random operands, which is to say never, which is to
/// say it is code nobody has ever executed.
///
/// Comparing before subtracting costs a multiply and a comparison per attempt — a constant factor
/// on an operation no JavaScript program runs in a loop — and every line of it is reached by
/// ordinary arithmetic. GOAL.md's preference for the boring implementation is exactly this trade:
/// the clever one is faster and has a branch that cannot be tested.
fn divide_magnitude(left: &[u32], right: &[u32]) -> Result<(Vec<u32>, Vec<u32>), Error> {
    // Normalise: shift both until the divisor's top bit is set. This is not a tidying step, it is
    // what makes the estimate below *close*. With a divisor whose top limb is 1, `head / top` can
    // be four billion times the true digit and the correction loop counts all the way down; with
    // the top bit set the estimate is at most two too large, which is why the loop is a loop and
    // not a search. Measured before it was here: a division of two three-limb numbers took a
    // second.
    //
    // **The shift can overflow the ceiling**, and the comment here used to say it could not: a
    // divisor already at `MAX_LIMBS` limbs whose top limb is dense needs one more to normalise, and
    // the trimmed result is then one past what this engine keeps. `unwrap_or_default` turned that
    // refusal into an *empty* divisor, and `divisor[n - 1]` with `n = 0` indexed `usize::MAX` — a
    // panic in the embedder's process from four tokens of script (GHSA-6976-qm5m-7mcj).
    //
    // So it is carried. §6.1.4 lets an implementation impose a limit and requires it to throw, and
    // a RangeError here is that: the division is refused rather than answered from a divisor that
    // is not the one that was written. Handling the spill limb inside the estimation would answer
    // it instead, and that is an algorithm change with a decision record in front of it.
    let shift = u64::from(right[right.len() - 1].leading_zeros());
    let divisor = shift_magnitude_left(right, shift)?;
    let dividend = shift_magnitude_left(left, shift)?;

    let n = divisor.len();
    let mut remainder: Vec<u32> = Vec::new();
    let mut quotient = vec![0u32; dividend.len()];
    for at in (0..dividend.len()).rev() {
        // Bring the next limb down, least significant first — so it goes to the *bottom* of the
        // running remainder and what is there already moves up one place.
        remainder.insert(0, dividend[at]);
        trim(&mut remainder);
        if compare_magnitude(&remainder, &divisor) == Ordering::Less {
            continue;
        }
        // The estimate, from one limb more of the remainder than the divisor has. Normalisation
        // makes it at most two too large and never too small, so the correction only comes down.
        let head = match remainder.len() > n {
            true => (u64::from(remainder[n]) << 32) | u64::from(remainder[n - 1]),
            false => u64::from(remainder[n - 1]),
        };
        let mut estimate = (head / u64::from(divisor[n - 1])).min(0xFFFF_FFFF);
        let mut product = multiply_by_limb(&divisor, estimate);
        // Try it, rather than subtracting optimistically and adding back when that went too far.
        // The add-back is Knuth's and it is correct; it also runs about once in two billion
        // divisions, which is to say it is code no test can reach. This runs on ordinary inputs.
        while compare_magnitude(&product, &remainder) == Ordering::Greater {
            estimate -= 1;
            product = multiply_by_limb(&divisor, estimate);
        }
        remainder = subtract_magnitude(&remainder, &product);
        quotient[at] = estimate as u32;
    }
    trim(&mut quotient);
    // The quotient is unaffected by the normalisation — both operands were scaled by the same
    // power of two — and the remainder was scaled with them, so it comes back.
    Ok((quotient, shift_magnitude_right(&remainder, shift)))
}

/// `magnitude * limb`, which long division needs to try a digit before committing to it.
fn multiply_by_limb(magnitude: &[u32], limb: u64) -> Vec<u32> {
    let mut product = Vec::new();
    let mut carry = 0u64;
    for value in magnitude {
        let total = u64::from(*value) * limb + carry;
        product.push(total as u32);
        carry = total >> 32;
    }
    while carry > 0 {
        product.push(carry as u32);
        carry >>= 32;
    }
    trim(&mut product);
    product
}

/// `magnitude << places`, in bits.
fn shift_magnitude_left(magnitude: &[u32], places: u64) -> Result<Vec<u32>, Error> {
    if magnitude.is_empty() {
        return Ok(Vec::new());
    }
    let limbs = (places / 32) as usize;
    let bits = (places % 32) as u32;
    // **The width is worked out before anything is allocated**, which is what lets one comparison
    // do the whole job. The shift adds a limb only when the top one has bits far enough up to
    // spill, so asking that directly gives the *final* length — where reserving a limb and
    // trimming it afterwards means the ceiling is applied to scratch space rather than to the
    // magnitude. That was this function's bug: a value landing exactly on the ceiling was refused,
    // `divide_magnitude` read the refusal as "cannot happen" and swallowed it with
    // `unwrap_or_default`, and an empty divisor indexed `divisor[n - 1]` at `usize::MAX`
    // (GHSA-6976-qm5m-7mcj).
    //
    // Computing it also removes the second bound the reserve needed. DR-0013's shape: ask before
    // the bytes are taken, rather than take them and notice.
    let top = magnitude[magnitude.len() - 1];
    let spills = bits > 0 && (top >> (32 - bits)) != 0;
    let width = magnitude.len() + limbs + usize::from(spills);
    within_ceiling(width)?;
    let mut shifted = vec![0u32; width];
    for (at, limb) in magnitude.iter().enumerate() {
        let wide = u64::from(*limb) << bits;
        shifted[at + limbs] |= wide as u32;
        // The high half of the last limb is what `spills` accounted for; when it did not, that
        // half is zero and there is no slot above to put it in.
        if at + limbs + 1 < width {
            shifted[at + limbs + 1] |= (wide >> 32) as u32;
        }
    }
    trim(&mut shifted);
    Ok(shifted)
}

/// `magnitude >> places`, in bits, discarding what falls off the bottom.
fn shift_magnitude_right(magnitude: &[u32], places: u64) -> Vec<u32> {
    let limbs = (places / 32) as usize;
    // No guard for a shift past the end: `limbs..magnitude.len()` is simply an empty range then,
    // and the answer is the empty magnitude either way.
    let bits = (places % 32) as u32;
    let mut shifted = Vec::new();
    for at in limbs..magnitude.len() {
        // `checked_shl` rather than `>>`, because a shift of a whole limb width is undefined in
        // Rust and this is reached with `bits` of zero on every whole-limb shift.
        let high = match bits {
            0 => 0,
            _ => magnitude
                .get(at + 1)
                .map_or(0, |next| next.wrapping_shl(32 - bits)),
        };
        shifted.push((magnitude[at] >> bits) | high);
    }
    trim(&mut shifted);
    shifted
}

/// Whether every bit `>> places` discards was already zero — see [`BigInt::shift_right`].
fn dropped_bits_were_zero(magnitude: &[u32], places: u64) -> bool {
    let limbs = (places / 32) as usize;
    let bits = (places % 32) as u32;
    if magnitude
        .iter()
        .take(limbs.min(magnitude.len()))
        .any(|limb| *limb != 0)
    {
        return false;
    }
    match bits {
        0 => true,
        _ => magnitude
            .get(limbs)
            .is_none_or(|limb| limb & ((1u32 << bits) - 1) == 0),
    }
}

#[cfg(test)]
mod tests;
