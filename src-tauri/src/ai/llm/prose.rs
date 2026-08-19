//! Keeps the trailing action out of the answer while the answer is still
//! arriving.
//!
//! On the tool track there is nothing to do: the prose is the whole of
//! `content` and the action travels in a field of its own. On the JSON track the
//! two share one stream and the object comes last -- so everything up to it is
//! painted as it decodes, and the object itself never is. A user watching
//! `{"action":"run","command":...` type itself out would be reading plumbing,
//! and the command card is about to say the same thing properly.
//!
//! Held back rather than filtered afterwards, because there is no afterwards
//! while it is streaming: what arrives is a prefix, and a prefix cannot be
//! parsed.
//!
//! Nothing here decides anything. `parse.rs` reads the reply once it is whole and
//! the panel repaints the finished bubble from that, so text held back too
//! eagerly comes back a moment later. The asymmetry is the whole design:
//! over-holding costs a redraw, under-holding paints an instruction at the user.

/// What an action object opens with. Nothing else in an answer does.
const OPENING: &str = "{\"action\"";

#[derive(Debug, Default)]
pub struct ProseStreamer {
    /// Characters from a `{` onwards that could still turn out to be [`OPENING`].
    holding: String,
    /// Set once the hold has proved to be an action object. Nothing further is
    /// painted: the rest of the reply belongs to the object.
    swallowed: bool,
}

impl ProseStreamer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the text to append to the thread, which is usually all of `chunk`.
    pub fn push(&mut self, chunk: &str) -> String {
        if self.swallowed {
            return String::new();
        }
        let mut out = String::new();

        for char in chunk.chars() {
            if self.holding.is_empty() && char != '{' {
                out.push(char);
                continue;
            }
            self.holding.push(char);

            /*
             * Whitespace is dropped before the comparison because `{ "action"` is
             * the same object as `{"action"`, and the model chooses. Safe this
             * early: the hold ends at the key, before any value the model wrote
             * could have whitespace that mattered.
             */
            let so_far: String = self.holding.chars().filter(|c| !c.is_whitespace()).collect();
            if so_far.starts_with(OPENING) {
                self.swallowed = true;
                self.holding.clear();
                return out;
            }
            if !OPENING.starts_with(&so_far) {
                // Brace-shaped prose after all -- `awk '{print $1}'`, a CSS rule,
                // somebody else's JSON. It was never painted, so it is painted
                // now, in one piece and in the order it was written.
                out.push_str(&self.holding);
                self.holding.clear();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(chunks: &[&str]) -> String {
        let mut streamer = ProseStreamer::new();
        chunks.iter().map(|chunk| streamer.push(chunk)).collect()
    }

    #[test]
    fn prose_is_handed_over_as_it_arrives() {
        let mut streamer = ProseStreamer::new();
        assert_eq!(streamer.push("The disk is "), "The disk is ");
        assert_eq!(streamer.push("nearly full."), "nearly full.");
    }

    #[test]
    fn the_action_at_the_end_is_never_painted() {
        assert_eq!(
            stream(&["I'll look.\n", r#"{"action":"run","#, r#""command":"df -h"}"#]),
            "I'll look.\n"
        );
    }

    /// The opening arrives a few characters at a time, and the decision has to
    /// survive being split anywhere inside it.
    #[test]
    fn an_opening_split_across_chunks_is_still_recognised() {
        assert_eq!(stream(&["ok ", "{", "\"a", "cti", "on\"", ":\"run\"}"]), "ok ");
    }

    #[test]
    fn a_space_inside_the_opening_does_not_fool_it() {
        assert_eq!(stream(&["ok ", "{ \"action\" : \"run\", \"command\":\"ls\"}"]), "ok ");
    }

    /// Braces are everywhere in an answer about a machine, and every one of them
    /// is text.
    #[test]
    fn brace_shaped_prose_is_released_once_it_proves_itself() {
        assert_eq!(stream(&["Use awk '{print $1}' here."]), "Use awk '{print $1}' here.");
        assert_eq!(stream(&["Post ", r#"{"user":"bob"}"#]), r#"Post {"user":"bob"}"#);
        assert_eq!(stream(&["a ", "{", "\"a", "ge\":3}"]), r#"a {"age":3}"#);
    }

    /// An answer that quotes the protocol mid-sentence loses the rest of itself
    /// until the repaint. Deliberate: the alternative is painting the opening of
    /// an instruction and taking it back, and this way round costs one redraw of
    /// text the panel is about to render properly anyway.
    #[test]
    fn an_example_mid_answer_holds_the_rest_for_the_repaint() {
        let mut streamer = ProseStreamer::new();
        assert_eq!(streamer.push("Send "), "Send ");
        assert_eq!(streamer.push(r#"{"action":"run"} and it runs."#), "");
    }

    #[test]
    fn a_whole_reply_pushed_at_once_still_comes_back() {
        assert_eq!(stream(&["All set.\n{\"action\":\"run\",\"command\":\"ls\"}"]), "All set.\n");
    }

    #[test]
    fn wide_characters_survive_a_chunk_boundary() {
        assert_eq!(stream(&["磁盘", "快满了"]), "磁盘快满了");
    }
}
