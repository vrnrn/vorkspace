import Foundation

struct DictationWord: Codable, Equatable {
    let heard: String
    let preferred: String
}

/// A single pass over Apple's output: exact phrases, whole words, longest first.
/// Rules never cascade, and no punctuation or sentence intent is invented.
struct DictationWords {
    private let expression: NSRegularExpression?
    private let spellings: [String: String]

    init(_ rules: [DictationWord]) throws {
        var spellings = [String: String]()
        for rule in rules {
            for phrase in [rule.heard, rule.preferred] {
                let key = Self.key(phrase)
                if let previous = spellings[key], previous != rule.preferred {
                    throw DictationError("Two custom words give different spellings for “\(phrase)”.")
                }
                spellings[key] = rule.preferred
            }
        }
        self.spellings = spellings
        let alternatives = spellings.keys.sorted {
            $0.count == $1.count ? $0 < $1 : $0.count > $1.count
        }.map { phrase in
            phrase.split(separator: " ").map { NSRegularExpression.escapedPattern(for: String($0)) }.joined(separator: "\\s+")
        }
        expression = alternatives.isEmpty ? nil : try NSRegularExpression(
            pattern: "(?<![\\p{L}\\p{M}\\p{N}_])(?:\(alternatives.joined(separator: "|")))(?![\\p{L}\\p{M}\\p{N}_])",
            options: [.caseInsensitive])
    }

    static func key(_ value: String) -> String {
        value.precomposedStringWithCanonicalMapping
            .split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
            .folding(options: .caseInsensitive, locale: Locale(identifier: "en_US_POSIX"))
    }

    func apply(_ source: String) -> String {
        let source = source.precomposedStringWithCanonicalMapping
        guard let expression else { return source }
        let text = source as NSString
        let result = NSMutableString(string: source)
        for match in expression.matches(in: source, range: NSRange(location: 0, length: text.length)).reversed() {
            if let spelling = spellings[Self.key(text.substring(with: match.range))] {
                result.replaceCharacters(in: match.range, with: spelling)
            }
        }
        return result as String
    }
}

struct DictationError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}

/// Settings and helper admission share the exact native dictionary validation.
@_cdecl("vorkspace_dictation_validate_words")
func dictationValidateWords(_ json: UnsafePointer<CChar>, _ reply: @convention(c) (UnsafePointer<CChar>?) -> Void) {
    do {
        let words = try JSONDecoder().decode([DictationWord].self, from: Data(String(cString: json).utf8))
        _ = try DictationWords(words)
        reply(nil)
    } catch { error.localizedDescription.withCString { reply($0) } }
}
