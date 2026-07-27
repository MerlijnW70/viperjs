//! The `ReservedWord` production of ECMA-262 §12.7.2, and nothing else.
//!
//! Its own file because the list *is* the point: 38 spellings a reader should be able to check
//! against the spec at a glance, without a punctuator table in the way.

/// The 38 spellings of `ReservedWord` (ECMA-262 §12.7.2).
///
/// This is the whole list and nothing more. `await` and `yield` are here because the production
/// lists them, even though §12.7.2 marks them contextually allowed as identifiers — that
/// exception is expressed by parameterized productions in §13.1, which is the parser's problem.
/// The five categories §12.7.2 describes collapse, for a lexer, into exactly one question: is
/// this spelling in the `ReservedWord` production? Contextual keywords (`let`, `static`,
/// `async`, `of`, `get`, `set`, `from`, `as`, `target`, `meta`) and the strict-mode future
/// reserved words (`implements`, `interface`, `package`, `private`, `protected`, `public`) are
/// deliberately absent: they are ordinary `IdentifierName`s until a grammatical context says
/// otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReservedWord {
    /// `await`
    Await,
    /// `break`
    Break,
    /// `case`
    Case,
    /// `catch`
    Catch,
    /// `class`
    Class,
    /// `const`
    Const,
    /// `continue`
    Continue,
    /// `debugger`
    Debugger,
    /// `default`
    Default,
    /// `delete`
    Delete,
    /// `do`
    Do,
    /// `else`
    Else,
    /// `enum` — reserved but unused; §12.7.2 Note 2 sets it aside for future extensions.
    Enum,
    /// `export`
    Export,
    /// `extends`
    Extends,
    /// `false`
    False,
    /// `finally`
    Finally,
    /// `for`
    For,
    /// `function`
    Function,
    /// `if`
    If,
    /// `import`
    Import,
    /// `in`
    In,
    /// `instanceof`
    Instanceof,
    /// `new`
    New,
    /// `null`
    Null,
    /// `return`
    Return,
    /// `super`
    Super,
    /// `switch`
    Switch,
    /// `this`
    This,
    /// `throw`
    Throw,
    /// `true`
    True,
    /// `try`
    Try,
    /// `typeof`
    Typeof,
    /// `var`
    Var,
    /// `void`
    Void,
    /// `while`
    While,
    /// `with`
    With,
    /// `yield`
    Yield,
}

impl ReservedWord {
    /// The one spelling this word has.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Await => "await",
            Self::Break => "break",
            Self::Case => "case",
            Self::Catch => "catch",
            Self::Class => "class",
            Self::Const => "const",
            Self::Continue => "continue",
            Self::Debugger => "debugger",
            Self::Default => "default",
            Self::Delete => "delete",
            Self::Do => "do",
            Self::Else => "else",
            Self::Enum => "enum",
            Self::Export => "export",
            Self::Extends => "extends",
            Self::False => "false",
            Self::Finally => "finally",
            Self::For => "for",
            Self::Function => "function",
            Self::If => "if",
            Self::Import => "import",
            Self::In => "in",
            Self::Instanceof => "instanceof",
            Self::New => "new",
            Self::Null => "null",
            Self::Return => "return",
            Self::Super => "super",
            Self::Switch => "switch",
            Self::This => "this",
            Self::Throw => "throw",
            Self::True => "true",
            Self::Try => "try",
            Self::Typeof => "typeof",
            Self::Var => "var",
            Self::Void => "void",
            Self::While => "while",
            Self::With => "with",
            Self::Yield => "yield",
        }
    }

    /// The reserved word `text` spells exactly, if it spells one.
    ///
    /// Only ever called with an escape-free `IdentifierName`, which is what makes a plain string
    /// comparison correct: §12.7.2 Note 1 says a keyword matches literal source characters, so
    /// `els\u{65}` must not reach here and be told it is `else`.
    pub fn from_text(text: &str) -> Option<Self> {
        Some(match text {
            "await" => Self::Await,
            "break" => Self::Break,
            "case" => Self::Case,
            "catch" => Self::Catch,
            "class" => Self::Class,
            "const" => Self::Const,
            "continue" => Self::Continue,
            "debugger" => Self::Debugger,
            "default" => Self::Default,
            "delete" => Self::Delete,
            "do" => Self::Do,
            "else" => Self::Else,
            "enum" => Self::Enum,
            "export" => Self::Export,
            "extends" => Self::Extends,
            "false" => Self::False,
            "finally" => Self::Finally,
            "for" => Self::For,
            "function" => Self::Function,
            "if" => Self::If,
            "import" => Self::Import,
            "in" => Self::In,
            "instanceof" => Self::Instanceof,
            "new" => Self::New,
            "null" => Self::Null,
            "return" => Self::Return,
            "super" => Self::Super,
            "switch" => Self::Switch,
            "this" => Self::This,
            "throw" => Self::Throw,
            "true" => Self::True,
            "try" => Self::Try,
            "typeof" => Self::Typeof,
            "var" => Self::Var,
            "void" => Self::Void,
            "while" => Self::While,
            "with" => Self::With,
            "yield" => Self::Yield,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::TokenKind;
    use crate::lexer::test_support::*;
    #[test]
    fn exactly_the_thirty_eight_reserved_words_lex_as_keywords() {
        use ReservedWord::*;
        // The §12.7.2 `ReservedWord` production, written out a third time — independently of
        // `as_str` and of `from_text`, which is what makes this a check rather than an echo.
        let all: &[(&str, ReservedWord)] = &[
            ("await", Await),
            ("break", Break),
            ("case", Case),
            ("catch", Catch),
            ("class", Class),
            ("const", Const),
            ("continue", Continue),
            ("debugger", Debugger),
            ("default", Default),
            ("delete", Delete),
            ("do", Do),
            ("else", Else),
            ("enum", Enum),
            ("export", Export),
            ("extends", Extends),
            ("false", False),
            ("finally", Finally),
            ("for", For),
            ("function", Function),
            ("if", If),
            ("import", Import),
            ("in", In),
            ("instanceof", Instanceof),
            ("new", New),
            ("null", Null),
            ("return", Return),
            ("super", Super),
            ("switch", Switch),
            ("this", This),
            ("throw", Throw),
            ("true", True),
            ("try", Try),
            ("typeof", Typeof),
            ("var", Var),
            ("void", Void),
            ("while", While),
            ("with", With),
            ("yield", Yield),
        ];
        assert_eq!(
            all.len(),
            38,
            "the ReservedWord production has 38 alternatives — recount before changing this"
        );
        let mut seen = std::collections::HashSet::new();
        for &(text, word) in all {
            assert_eq!(word.as_str(), text);
            assert_eq!(ReservedWord::from_text(text), Some(word));
            assert!(seen.insert(word), "{text:?} appears twice");
            assert_eq!(
                kinds(text),
                [TokenKind::Keyword(word), TokenKind::Eof],
                "lexing {text:?}"
            );
        }

        // Contextual keywords are NOT reserved words. Lexing them as keywords would make
        // `var let = 1` a syntax error, which it is not outside strict mode, and would take the
        // decision away from the parser that actually has the context (§12.7.2's five
        // categories). The strict-mode future reserved words belong to the parser too.
        for text in [
            "let",
            "static",
            "async",
            "of",
            "get",
            "set",
            "from",
            "as",
            "target",
            "meta",
            "implements",
            "interface",
            "package",
            "private",
            "protected",
            "public",
            "arguments",
            "eval",
            "undefined",
            "NaN",
        ] {
            assert_eq!(ReservedWord::from_text(text), None, "{text:?}");
            assert_eq!(kinds(text), [PLAIN, TokenKind::Eof], "lexing {text:?}");
        }
        // Near misses: a prefix, an extension, and a case change are all just names.
        for text in ["awai", "awaits", "Await", "iff", "i", "IF", "in_"] {
            assert_eq!(ReservedWord::from_text(text), None, "{text:?}");
            assert_eq!(kinds(text), [PLAIN, TokenKind::Eof], "lexing {text:?}");
        }
        // A keyword still ends where the name ends.
        assert_eq!(
            kinds("if(a)"),
            [
                TokenKind::Keyword(If),
                TokenKind::LParen,
                PLAIN,
                TokenKind::RParen,
                TokenKind::Eof
            ]
        );
    }
}
