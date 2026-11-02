// Adapted from FluidAudio 386f7195670b77f2954c7ee143ec63fcb50b6c3b (Apache-2.0).
// See licenses/LICENSE-FLUIDAUDIO-APACHE and NOTICE.
import Foundation

public class Tokenizer {
    private var vocab: [String: String] = [:]
    private var idToToken: [Int: String] = [:]

    public init(vocabPath: URL) throws {
        let data = try Data(contentsOf: vocabPath)
        guard let json = try JSONSerialization.jsonObject(with: data, options: []) as? [String: String] else { throw EouError.invalidVocab }

        self.vocab = json
        for (key, value) in json {
            if let id = Int(key) {
                self.idToToken[id] = value
            }
        }
    }

    public func decode(ids: [Int], skipSpecialTokens: Bool = false) -> String {
        var text = ""
        for id in ids {
            if let token = idToToken[id] {
                if skipSpecialTokens && token.hasPrefix("<") && token.hasSuffix(">") {
                    continue
                }
                text += token
            }
        }
        // Replace SentencePiece word boundary marker with space, then trim
        return text.replacingOccurrences(of: "\u{2581}", with: " ")
            .trimmingCharacters(in: .whitespaces)
    }
}
