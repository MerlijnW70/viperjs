//! What a test262 file says about itself.
//!
//! Every test carries a YAML block between `/*---` and `---*/`, and it is not decoration: it says
//! whether the file is a script or a module, whether it must be run in strict mode or must not be,
//! which harness files it needs, and — for a test of an error — what the error must be and when.
//! Running a test without reading it is running a different test.
//!
//! # Why the YAML is read by hand
//!
//! Because it is not YAML. It is the handful of shapes `INTERPRETING.md` documents — a few scalar
//! keys, two list-valued ones, one nested map for `negative` — and a real parser would be a
//! dependency for a file format this harness controls entirely. What is here refuses what it does
//! not understand rather than guessing, which is the property that matters: a test whose
//! frontmatter is misread is a test that reports the wrong thing.

use std::collections::BTreeSet;

/// What a test262 file declares about itself.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    /// One line of prose, used to name a failure.
    pub description: String,
    /// The harness files this test needs, in the order it named them.
    ///
    /// `assert.js` and `sta.js` are *not* here and are included anyway — `INTERPRETING.md` says
    /// every test gets them unless it is `raw`.
    pub includes: Vec<String>,
    /// The flags, as they were written.
    pub flags: BTreeSet<String>,
    /// What must go wrong, for a test that is about something going wrong.
    pub negative: Option<Negative>,
    /// The features the test needs, which is how a harness skips what an engine has not built.
    pub features: BTreeSet<String>,
}

/// A test that must fail, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negative {
    /// When it must fail — `parse`, `early`, `resolution` or `runtime`.
    ///
    /// The distinction is the whole point of the key. A `parse` failure that happens at run time
    /// is a bug even though the test "failed" as asked, because the program should never have
    /// begun.
    pub phase: String,
    /// The constructor of the error, by name: `SyntaxError`, `TypeError`, and so on.
    pub kind: String,
}

impl Frontmatter {
    /// Read the block out of a test file.
    ///
    /// Answers `None` for a file with no block at all, which `INTERPRETING.md` allows only for
    /// files under `harness/` — every test has one.
    pub fn parse(source: &str) -> Option<Self> {
        let start = source.find("/*---")? + "/*---".len();
        let end = source[start..].find("---*/")? + start;
        let mut block = Self::default();
        let mut lines = source[start..end].lines().peekable();
        while let Some(line) = lines.next() {
            let (key, value) = match line.split_once(':') {
                // A continuation of a folded scalar, or a blank line inside one. The keys this
                // harness reads are never folded, so skipping is right and not a loss.
                None => continue,
                Some(pair) => pair,
            };
            // Only a key at the left margin is a key. An indented one belongs to whatever is
            // above it — which is how `negative`'s two lines are told from two top-level keys.
            if key.starts_with(char::is_whitespace) {
                continue;
            }
            let value = value.trim();
            match key.trim() {
                "description" => block.description = value.to_string(),
                "includes" => block.includes = read_list(value, &mut lines),
                "flags" => block.flags = read_list(value, &mut lines).into_iter().collect(),
                "features" => block.features = read_list(value, &mut lines).into_iter().collect(),
                "negative" => block.negative = read_negative(&mut lines),
                // `info`, `es5id`, `esid`, `author` and the rest say nothing about how to run it.
                _ => {}
            }
        }
        Some(block)
    }

    /// Whether the flags say this.
    pub fn has(&self, flag: &str) -> bool {
        self.flags.contains(flag)
    }
}

/// A list written either inline as `[a, b]` or as indented `- a` lines beneath the key.
fn read_list<'a>(
    inline: &str,
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Vec<String> {
    if let Some(items) = inline
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return items
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
    }
    let mut items = Vec::new();
    // The `- ` is the whole delimiter. YAML lets a sequence sit at its key's own indentation as
    // well as under it, so an indentation test here would refuse a list that is written correctly
    // — and the first line without a `- ` ends the list either way.
    while let Some(item) = lines
        .peek()
        .and_then(|line| line.trim_start().strip_prefix("- "))
    {
        items.push(item.trim().to_string());
        lines.next();
    }
    items
}

/// The two indented lines beneath a `negative:` key.
fn read_negative<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Option<Negative> {
    let mut phase = None;
    let mut kind = None;
    while let Some(line) = lines.peek() {
        if !line.starts_with(char::is_whitespace) {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            break;
        };
        match key.trim() {
            "phase" => phase = Some(value.trim().to_string()),
            "type" => kind = Some(value.trim().to_string()),
            _ => break,
        }
        lines.next();
    }
    Some(Negative {
        phase: phase?,
        kind: kind?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_read_out_of_the_comment_and_nothing_else_is() {
        let source = "// a copyright line\n/*---\ndescription: what it checks\nes5id: 1.2.3\n---*/\nthrow 1;";
        let block = Frontmatter::parse(source).expect("there is a block");
        assert_eq!(block.description, "what it checks");
        // `es5id` and the rest say nothing about how to run the test, so they are read past
        // rather than refused: this harness understands the keys it acts on.
        assert!(block.flags.is_empty());
        assert!(block.negative.is_none());
        // A file with no block at all is not a test — only `harness/` files have none.
        assert!(Frontmatter::parse("throw 1;").is_none());
    }

    #[test]
    fn a_list_is_read_in_both_of_the_shapes_test262_writes_it() {
        let inline = "/*---\nincludes: [assert.js, compareArray.js]\nflags: [onlyStrict]\n---*/";
        let block = Frontmatter::parse(inline).expect("a block"); // the test is about it
        assert_eq!(block.includes, ["assert.js", "compareArray.js"]);
        assert!(block.has("onlyStrict"));

        let indented =
            "/*---\nincludes:\n  - assert.js\n  - propertyHelper.js\nflags:\n  - noStrict\n---*/";
        let block = Frontmatter::parse(indented).expect("a block"); // same
        assert_eq!(block.includes, ["assert.js", "propertyHelper.js"]);
        assert!(block.has("noStrict"));
        assert!(!block.has("onlyStrict"));

        // YAML lets a sequence sit at its key's own indentation, and a list written that way is
        // written correctly. The `- ` is the delimiter; where it starts on the line is not.
        let flush = "/*---\nincludes:\n- assert.js\n- compareArray.js\ndescription: after\n---*/";
        let block = Frontmatter::parse(flush).expect("a block"); // same
        assert_eq!(block.includes, ["assert.js", "compareArray.js"]);
        // …and the first line without a `- ` ends the list rather than joining it.
        assert_eq!(block.description, "after");
    }

    #[test]
    fn a_negative_test_says_when_it_must_fail_as_well_as_how() {
        // The phase is the half that is easy to drop, and dropping it turns a test of the parser
        // into a test that something, somewhere, went wrong.
        let source = "/*---\nnegative:\n  phase: parse\n  type: SyntaxError\nflags: [raw]\n---*/";
        let block = Frontmatter::parse(source).expect("a block"); // the test is about it
        let negative = block.negative.as_ref().expect("a negative block"); // same
        assert_eq!(negative.phase, "parse");
        assert_eq!(negative.kind, "SyntaxError");
        assert!(block.has("raw"));

        // A `negative` block is nested, and two keys at the margin are two keys rather than the
        // inside of the block above them. Read otherwise, a file could acquire an expectation of
        // failure it never wrote.
        let flat = "/*---\nnegative:\nphase: parse\ntype: SyntaxError\n---*/";
        assert!(
            Frontmatter::parse(flat)
                .expect("a block")
                .negative
                .is_none()
        ); // same

        // Half a negative block is not one. A test that says it must fail without saying how
        // cannot be checked, and reading it as "any failure will do" would pass on the wrong one.
        let half = "/*---\nnegative:\n  phase: parse\n---*/";
        assert!(
            Frontmatter::parse(half)
                .expect("a block")
                .negative
                .is_none()
        ); // same
    }

    #[test]
    fn an_indented_key_belongs_to_what_is_above_it() {
        // `info:` blocks are folded prose and routinely contain lines with colons in them. Read
        // as keys, they would overwrite the real ones — so only a key at the left margin counts.
        let source = "/*---\ninfo: |\n  description: this is prose, not a key\n  flags: [onlyStrict]\ndescription: the real one\n---*/";
        let block = Frontmatter::parse(source).expect("a block"); // the test is about it
        assert_eq!(block.description, "the real one");
        assert!(block.flags.is_empty());
    }
}
