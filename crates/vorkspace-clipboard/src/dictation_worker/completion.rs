//! A narrow text-only question hint for an explicit Unified English Stop.
//! It has no access to prosody and cannot determine intent.

pub(super) fn suggest(text: &str) -> Option<char> {
    let text = text.trim();
    if text.len() > 512 || !text.as_bytes().last()?.is_ascii_alphabetic() {
        return None;
    }

    // Only cross a conservative-looking sentence boundary. A dot inside a token, a
    // title/initial, a short abbreviation, or a symbol may be a URL or code.
    // Refusing uncertain prefixes avoids reinterpreting an abbreviation as a
    // sentence ending; no part of the prefix is ever rewritten.
    let mut boundary = 0;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b' ' => {}
            b',' if text.as_bytes().get(index + 1) == Some(&b' ') => {}
            b'.' | b'?' | b'!' if text.as_bytes().get(index + 1) == Some(&b' ') => {
                if byte == b'.' {
                    let previous = text[..index]
                        .rsplit(|c: char| !c.is_ascii_alphabetic())
                        .next()?;
                    if previous.len() < 4
                        || !previous.bytes().all(|c| c.is_ascii_lowercase())
                        || matches!(previous, "approx" | "dept" | "misc" | "prof")
                    {
                        return None;
                    }
                }
                boundary = index + 1;
            }
            _ => return None,
        }
    }

    // Reject technical and numeric input throughout, not just in the final
    // clause. The words here are boundary hints, not a grammar or language ID.
    let mut previous = String::new();
    for word in text
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
    {
        let lower = word.to_ascii_lowercase();
        if word.bytes().skip(1).any(|c| c.is_ascii_uppercase())
            || excluded(&lower)
            || (lower == "case" && matches!(previous.as_str(), "camel" | "snake" | "kebab"))
        {
            return None;
        }
        previous = lower;
    }

    let sentence = text[boundary..].trim();
    let clause = if let Some((lead, rest)) = sentence.split_once(',') {
        // Only independent conversational openers are safe here. In
        // particular, "I wonder, ..." can introduce an indirect question.
        if !matches!(
            lead.trim().to_ascii_lowercase().as_str(),
            "hi" | "hi there"
                | "hello"
                | "hello there"
                | "hey"
                | "hey there"
                | "excuse me"
                | "please"
                | "well"
                | "okay"
                | "thanks"
                | "by the way"
        ) || rest.contains(',')
        {
            return None;
        }
        rest.trim()
    } else {
        sentence
    };
    let words: Vec<_> = clause
        .split_ascii_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    if !(3..=24).contains(&words.len()) || dangling(words.last()?.as_str()) {
        return None;
    }
    let at = |index: usize| words.get(index).map(String::as_str).unwrap_or("");

    // WH + auxiliary + an explicit subject is an inverted direct-question
    // shape. Free relatives such as "what is important matters" do not have
    // this shape. A short nominal form uses a few common noun heads: an
    // arbitrary last word could be an unfinished modifier ("your current").
    let wh = matches!(at(0), "what" | "where" | "when" | "why" | "how" | "who");
    let wh_nominal = wh
        && copula(at(1))
        && words.len() == 4
        && determiner(at(2))
        && matches!(
            at(3),
            "name"
                | "age"
                | "address"
                | "answer"
                | "idea"
                | "job"
                | "plan"
                | "reason"
                | "time"
                | "title"
        );
    // A trailing determiner followed by one word may still be an unfinished
    // noun phrase ("the new ..."). Only the short inverted WH form is an
    // exception, because its head has been recognized above.
    if determiner(at(words.len() - 2)) && !wh_nominal {
        return None;
    }
    let wh_inverted = wh
        && question_auxiliary(at(1))
        && (copula(at(1)) || words.len() >= 4)
        && (subject(at(2)) || wh_nominal);

    // Avoid conditional inversions such as "should you need help ..." and
    // "had we known ...". With a noun subject, accept only a small, visibly
    // bounded copular predicate; otherwise a phrase like "is the old chair"
    // can be an unfinished noun phrase.
    let inverted = question_auxiliary(at(0))
        && (subject(at(1))
            || (copula(at(0))
                && words.len() == 4
                && determiner(at(1))
                && (at(3).ends_with("ing")
                    || at(3).ends_with("ed")
                    || matches!(at(3), "ready" | "open" | "closed"))));

    let tail_start = if wh_inverted { 3 } else { 2 };
    ((wh_inverted || inverted) && !words[tail_start..].iter().any(|w| auxiliary(w))).then_some('?')
}

fn subject(word: &str) -> bool {
    matches!(
        word,
        "i" | "you" | "we" | "he" | "she" | "it" | "they" | "this" | "that"
    )
}

fn determiner(word: &str) -> bool {
    matches!(
        word,
        "the" | "a" | "my" | "your" | "his" | "her" | "our" | "their"
    )
}

fn copula(word: &str) -> bool {
    matches!(word, "is" | "are" | "was")
}

fn question_auxiliary(word: &str) -> bool {
    copula(word)
        || matches!(
            word,
            "am" | "do" | "does" | "did" | "can" | "could" | "will" | "would"
        )
}

fn auxiliary(word: &str) -> bool {
    question_auxiliary(word)
        || matches!(
            word,
            "were" | "should" | "shall" | "may" | "might" | "must" | "has" | "have" | "had"
        )
}

fn dangling(word: &str) -> bool {
    word.ends_with("ly")
        || auxiliary(word)
        || matches!(
            word,
            "a" | "an"
                | "the"
                | "my"
                | "your"
                | "his"
                | "her"
                | "our"
                | "their"
                | "to"
                | "of"
                | "for"
                | "with"
                | "at"
                | "by"
                | "from"
                | "in"
                | "on"
                | "into"
                | "about"
                | "up"
                | "out"
                | "off"
                | "over"
                | "under"
                | "through"
                | "and"
                | "or"
                | "but"
                | "because"
                | "if"
                | "whether"
                | "what"
                | "which"
                | "who"
                | "whose"
                | "where"
                | "when"
                | "why"
                | "how"
                | "not"
                | "very"
                | "favorite"
                | "preferred"
                | "best"
                | "next"
                | "almost"
                | "supposed"
                | "be"
                | "been"
                | "being"
                | "take"
                | "make"
                | "send"
                | "give"
                | "put"
                | "tell"
                | "bring"
                | "show"
                | "choose"
                | "need"
                | "want"
        )
}

fn excluded(word: &str) -> bool {
    matches!(
        word,
        "zero"
            | "one"
            | "two"
            | "three"
            | "four"
            | "five"
            | "six"
            | "seven"
            | "eight"
            | "nine"
            | "ten"
            | "eleven"
            | "twelve"
            | "thirteen"
            | "fourteen"
            | "fifteen"
            | "sixteen"
            | "seventeen"
            | "eighteen"
            | "nineteen"
            | "twenty"
            | "thirty"
            | "forty"
            | "fifty"
            | "sixty"
            | "seventy"
            | "eighty"
            | "ninety"
            | "hundred"
            | "thousand"
            | "million"
            | "billion"
            | "trillion"
            | "first"
            | "second"
            | "third"
            | "fourth"
            | "fifth"
            | "sixth"
            | "seventh"
            | "eighth"
            | "ninth"
            | "tenth"
            | "eleventh"
            | "twelfth"
            | "thirteenth"
            | "fourteenth"
            | "fifteenth"
            | "sixteenth"
            | "seventeenth"
            | "eighteenth"
            | "nineteenth"
            | "twentieth"
            | "thirtieth"
            | "fortieth"
            | "fiftieth"
            | "sixtieth"
            | "seventieth"
            | "eightieth"
            | "ninetieth"
            | "hundredth"
            | "thousandth"
            | "millionth"
            | "billionth"
            | "trillionth"
            | "half"
            | "halves"
            | "quarter"
            | "quarters"
            | "dozen"
            | "dozens"
            | "number"
            | "digit"
            | "digits"
            | "url"
            | "http"
            | "https"
            | "www"
            | "dot"
            | "slash"
            | "underscore"
            | "identifier"
            | "variable"
            | "function"
            | "command"
            | "terminal"
            | "code"
            | "api"
            | "json"
            | "sql"
            | "git"
            | "github"
            | "cargo"
            | "npm"
            | "python"
            | "javascript"
            | "rust"
            | "docker"
            | "kubectl"
            | "localhost"
    )
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
