//! Why a decision was made, recorded so it can be checked later.
//!
//! The review's §17 asks that every important decision be able to answer why it was
//! produced, why that unit and that target were chosen, and why the alternatives were
//! not. That is not documentation - it is the thing this session kept needing and did not
//! have. Six rounds of v2 work went into tuning numbers that moved the ledger by nothing,
//! because there was no way to see which candidate lost and by how much; the decision was
//! a fold over a list nobody printed.
//!
//! So the shape here is: a decision names its subject, its options with their scores, the
//! winner, and a reason. The reason is the part that cannot be derived from the numbers -
//! "cheapest that answers anti-air" is a judgement, not an argmax.
//!
//! Records are plain values, bounded, and carry the frame they were made at. Nothing here
//! feeds back into play.

use std::collections::BTreeMap;

/// One option a decision considered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What was on offer: a type name, an object id, a cell - whatever the decision is
    /// about, named so a reader can look it up.
    pub label: String,
    /// The number the decision ranked it by. Signed, because some scores are penalties.
    pub score: i64,
    /// Why it lost, when it did. Empty for the winner.
    pub rejected_because: String,
}

/// One decision, with what it chose and what it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The frame it was made at: every claim here is about that frame.
    pub frame: u64,
    /// Which part of the automated player decided, e.g. `production` or `target`.
    pub subsystem: &'static str,
    /// What the decision was about - a house, an object id, a cell.
    pub subject: String,
    /// Everything considered, in the order it was considered. The winner is one of these;
    /// keeping the losers is the point, since a decision with one candidate is not a
    /// decision.
    pub candidates: Vec<Candidate>,
    /// The label of whatever won, or `None` when nothing was chosen.
    pub chosen: Option<String>,
    /// Why, in the decider's own words. A judgement, not a restatement of the score.
    pub reason: String,
}

impl Decision {
    /// The winning candidate, if one was chosen.
    pub fn winner(&self) -> Option<&Candidate> {
        let chosen = self.chosen.as_deref()?;
        self.candidates.iter().find(|c| c.label == chosen)
    }

    /// One line, for a log or a test failure message.
    pub fn describe(&self) -> String {
        let chosen = self.chosen.as_deref().unwrap_or("nothing");
        format!(
            "frame {} {} for {}: chose {chosen} of {} ({})",
            self.frame,
            self.subsystem,
            self.subject,
            self.candidates.len(),
            self.reason,
        )
    }
}

/// How many decisions are kept.
///
/// A match makes many per frame and an unbounded log is a leak. Oldest are dropped, so a
/// long match keeps the recent reasoning, which is what a post-mortem actually reads.
pub const CAPACITY: usize = 512;

/// The decision record for one match.
///
/// Belongs to the match, not to the process: the review's principle G is that nothing
/// survives a match, and a trace carried between matches would attribute one game's
/// reasoning to another.
#[derive(Debug, Default)]
pub struct Trace {
    decisions: Vec<Decision>,
    /// How many were dropped for want of room, so a reader knows the log is a window and
    /// not the whole story.
    dropped: usize,
    /// Per-subsystem counts, so "production decided nothing all match" is visible without
    /// reading every record.
    by_subsystem: BTreeMap<&'static str, usize>,
}

impl Trace {
    /// An empty trace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one decision, dropping the oldest if full.
    pub fn record(&mut self, decision: Decision) {
        if self.decisions.len() >= CAPACITY {
            self.decisions.remove(0);
            self.dropped += 1;
        }
        *self.by_subsystem.entry(decision.subsystem).or_insert(0) += 1;
        self.decisions.push(decision);
    }

    /// Every decision still held, oldest first.
    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// How many were dropped to stay within [`CAPACITY`].
    pub const fn dropped(&self) -> usize {
        self.dropped
    }

    /// How many decisions each subsystem made.
    pub fn counts(&self) -> &BTreeMap<&'static str, usize> {
        &self.by_subsystem
    }

    /// The last decision about `subject`, if any.
    ///
    /// The question the review's §17 actually asks - why was *this* chosen - is almost
    /// always about the most recent decision on a subject.
    pub fn last_about(&self, subject: &str) -> Option<&Decision> {
        self.decisions
            .iter()
            .rev()
            .find(|decision| decision.subject == subject)
    }

    /// A structured dump: one line per decision, winners and losers both.
    ///
    /// Plain text rather than JSON because the readers so far are a person and a test
    /// assertion, and both prefer a line they can grep to a document they must parse. If a
    /// machine consumer appears, this is the function to grow a second form beside.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "trace: {} decisions, {} dropped, by subsystem {:?}\n",
            self.decisions.len(),
            self.dropped,
            self.by_subsystem,
        ));
        for decision in &self.decisions {
            out.push_str(&decision.describe());
            out.push('\n');
            for candidate in &decision.candidates {
                let mark = if decision.chosen.as_deref() == Some(candidate.label.as_str()) {
                    ">"
                } else {
                    " "
                };
                out.push_str(&format!(
                    "  {mark} {:<20} {:<8} {}\n",
                    candidate.label, candidate.score, candidate.rejected_because,
                ));
            }
        }
        out
    }
}
