/// What the listener was doing while a track played.
///
/// A closed set, the same four words the phone and the desktop use: "the tracks I
/// finish while gaming" is a question worth asking later, and free text would make it
/// unanswerable within a week. Written when the listen *begins*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Activity {
    /// Playing a game.
    Gaming,
    /// Working or studying.
    Focus,
    /// Housework.
    Chores,
    /// Travelling to or from somewhere.
    Commute,
}

impl Activity {
    /// Every activity.
    pub const ALL: [Activity; 4] = [Self::Gaming, Self::Focus, Self::Chores, Self::Commute];

    /// The lower-case word stored in `PlayEvent::activity`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gaming => "gaming",
            Self::Focus => "focus",
            Self::Chores => "chores",
            Self::Commute => "commute",
        }
    }

    /// The word to show, capitalised.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Gaming => "Gaming",
            Self::Focus => "Focus",
            Self::Chores => "Chores",
            Self::Commute => "Commute",
        }
    }

    /// Anything outside the set is dropped rather than stored: a newer build's word
    /// would otherwise sit in a column no query can interpret. Exact match.
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        Self::ALL
            .into_iter()
            .find(|activity| activity.as_str() == raw)
    }
}
