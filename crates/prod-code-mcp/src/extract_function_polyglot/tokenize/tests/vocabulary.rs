/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn reserved_words_are_not_extraction_inputs() {
    use Language::*;
    let cases = [
        (
            Python,
            "and as assert async await break case class continue def del elif else except finally for from global if import in is lambda match nonlocal not or pass raise return try while with yield True False None self",
        ),
        (
            TypeScript,
            "break case catch class const continue debugger default delete do else export extends finally for function if import in instanceof new return super switch this throw try typeof var void while with yield let static enum await async from as true false null undefined number string boolean any never unknown type interface implements declare readonly",
        ),
        (
            JavaScript,
            "break case catch class const continue debugger default delete do else export extends finally for function if import in instanceof new return super switch this throw try typeof var void while with yield let static enum await async from as true false null undefined number string boolean any never unknown type interface implements declare readonly",
        ),
        (
            Go,
            "break default func interface select case defer go map struct chan else goto package switch const fallthrough if range type continue for import return var true false nil int int8 int16 int32 int64 uint uint8 uint16 uint32 uint64 string float32 float64 bool byte rune error any",
        ),
        (
            C,
            "auto break case char const continue default do double else enum extern float for goto if int long register return short signed sizeof static struct switch typedef union unsigned void volatile while class namespace new delete this public private protected virtual override template typename using true false nullptr std bool size_t constexpr inline",
        ),
        (
            Cpp,
            "auto break case char const continue default do double else enum extern float for goto if int long register return short signed sizeof static struct switch typedef union unsigned void volatile while class namespace new delete this public private protected virtual override template typename using true false nullptr std bool size_t constexpr inline",
        ),
        (
            Swift,
            "associatedtype class deinit enum extension fileprivate func import init inout internal let open operator private protocol public rethrows static struct subscript typealias var break case continue default defer do else fallthrough for guard if in repeat return switch where while as Any catch false is nil super self Self throw throws true try Int Double Float String Bool",
        ),
        (
            Rust,
            "as break const continue crate else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while async await dyn",
        ),
        (
            Java,
            "abstract assert boolean break byte case catch char class const continue default do double else enum extends final finally float for goto if implements import instanceof int interface long native new package private protected public return short static strictfp super switch synchronized this throw throws transient try void volatile while record var yield sealed permits non-sealed true false null",
        ),
        (
            Kotlin,
            "as break class continue do else false for fun if in interface is null object package return super this throw true try typealias typeof val var when while by catch constructor delegate dynamic field file finally get import init param property receiver set setparam where actual abstract annotation companion const crossinline data enum expect external final infix inline inner internal lateinit noinline open operator out override private protected public reified sealed suspend tailrec vararg value",
        ),
        (
            Csharp,
            "abstract as base bool break byte case catch char checked class const continue decimal default delegate do double else enum event explicit extern false finally fixed float for foreach goto if implicit in int interface internal is lock long namespace new null object operator out override params private protected public readonly record ref return sbyte sealed short sizeof stackalloc static string struct switch this throw true try typeof uint ulong unchecked unsafe ushort using virtual void volatile while var async await yield",
        ),
        (
            Zig,
            "addrspace align allowzero and anyframe anytype asm async await break callconv catch comptime const continue defer else enum errdefer error export extern fn for if inline noalias noinline nosuspend opaque or orelse packed pub resume return linksection struct suspend switch test threadlocal try union unreachable usingnamespace var volatile while true false null undefined u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize f16 f32 f64 f80 f128 c_int c_uint c_long c_ulong c_char bool void noreturn type anyerror",
        ),
    ];
    for (lang, words) in cases {
        for word in words.split_whitespace() {
            assert!(is_keyword(word, lang), "{lang:?}: {word}");
            assert!(
                !is_keyword(&format!("{word}_local"), lang),
                "{lang:?}: {word}_local"
            );
        }
        for word in ["", "local_value", "λ", "RETURN"] {
            assert!(!is_keyword(word, lang), "{lang:?}: {word}");
        }
    }
}

#[test]
fn builtins_are_language_specific_and_match_whole_names() {
    use Language::*;
    let cases = [
        (
            Python,
            "print len range sum min max abs round int float str bool list dict set tuple open isinstance issubclass type zip map filter enumerate any all sorted reversed id repr iter next",
        ),
        (
            TypeScript,
            "console Math JSON Object Array String Number Boolean Date RegExp Error Promise Map Set WeakMap WeakSet Symbol parseInt parseFloat isNaN isFinite encodeURI decodeURI",
        ),
        (
            JavaScript,
            "console Math JSON Object Array String Number Boolean Date RegExp Error Promise Map Set WeakMap WeakSet Symbol parseInt parseFloat isNaN isFinite encodeURI decodeURI",
        ),
        (
            Go,
            "fmt make append copy delete len cap panic recover close new real imag complex print println",
        ),
        (
            C,
            "std printf scanf cout cin cerr endl malloc free sizeof memcpy memset vector string map unordered_map set shared_ptr unique_ptr",
        ),
        (
            Cpp,
            "std printf scanf cout cin cerr endl malloc free sizeof memcpy memset vector string map unordered_map set shared_ptr unique_ptr",
        ),
        (
            Swift,
            "print min max abs Array Dictionary Set String Int Double Float Bool fatalError precondition",
        ),
        (
            Rust,
            "println print eprintln eprint format vec panic Some None Ok Err Box Vec String Option Result",
        ),
        (
            Java,
            "System String Object Integer Long Double Float Boolean Character Byte Short Math Arrays Collections List Map Set Optional Objects StringBuilder StringBuffer Exception RuntimeException Throwable Thread Runnable",
        ),
        (
            Kotlin,
            "println print require check error assert TODO run let also apply with takeIf takeUnless repeat lazy emptyList listOf mutableListOf emptySet setOf mutableSetOf emptyMap mapOf mutableMapOf String Int Long Double Float Boolean Byte Short Char Any Unit Nothing Array ByteArray IntArray LongArray CharArray BooleanArray",
        ),
        (
            Csharp,
            "Console String Object Int32 Int64 Double Single Boolean Char Byte Int16 Math Array List Dictionary HashSet StringBuilder Exception Task ValueTask Action Func Predicate Nullable Span ReadOnlySpan Memory",
        ),
        (Zig, "std builtin root assert"),
    ];
    for (lang, words) in cases {
        for word in words.split_whitespace() {
            assert!(is_builtin_or_global(word, lang), "{lang:?}: {word}");
            assert!(!is_builtin_or_global(&format!("{word}_local"), lang));
        }
        for word in ["", "local_value", "λ"] {
            assert!(!is_builtin_or_global(word, lang));
        }
    }
    assert!(!is_builtin_or_global("Console", Python));
    assert!(!is_builtin_or_global("console", Csharp));
    assert!(!is_keyword("def", Rust));
    assert!(!is_keyword("fn", Python));
}
