//! The content area's back stack.
//!
//! A sidebar section is a root; opening an album or an artist pushes onto it;
//! Back pops. Pages below the top keep their state, so going back lands where
//! the list was left, not at a freshly loaded one.

/// A stack that is never empty: the root stays when everything above it pops.
#[derive(Debug, Clone, PartialEq)]
pub struct BackStack<T> {
    entries: Vec<T>,
}

impl<T> BackStack<T> {
    pub fn new(root: T) -> Self {
        Self {
            entries: vec![root],
        }
    }

    /// What is showing.
    pub fn current(&self) -> &T {
        // Never empty: `new` puts the root in and `back` never takes it out.
        self.entries.last().expect("a back stack is never empty")
    }

    #[cfg(test)]
    pub fn current_mut(&mut self) -> &mut T {
        self.entries
            .last_mut()
            .expect("a back stack is never empty")
    }

    #[cfg(test)]
    pub fn root(&self) -> &T {
        &self.entries[0]
    }

    pub fn push(&mut self, page: T) {
        self.entries.push(page);
    }

    /// Goes back one page. `false`, and nothing changes, at the root.
    pub fn back(&mut self) -> bool {
        if self.entries.len() > 1 {
            self.entries.pop();
            true
        } else {
            false
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.entries.len() > 1
    }

    /// Starts over at a new root: a sidebar click.
    pub fn reset(&mut self, root: T) {
        self.entries.clear();
        self.entries.push(root);
    }

    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushing_and_going_back() {
        let mut stack = BackStack::new("albums");
        assert!(!stack.can_go_back());
        stack.push("album");
        stack.push("artist");
        assert_eq!(*stack.current(), "artist");
        assert_eq!(stack.depth(), 3);
        assert!(stack.back());
        assert_eq!(*stack.current(), "album");
        assert!(stack.back());
        assert_eq!(*stack.current(), "albums");
    }

    #[test]
    fn the_root_never_pops() {
        let mut stack = BackStack::new("home");
        assert!(!stack.back());
        assert_eq!(*stack.current(), "home");
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn a_sidebar_click_starts_over() {
        let mut stack = BackStack::new("home");
        stack.push("album");
        stack.reset("songs");
        assert_eq!(*stack.current(), "songs");
        assert_eq!(*stack.root(), "songs");
        assert!(!stack.can_go_back());
    }

    #[test]
    fn pages_below_keep_their_state() {
        let mut stack = BackStack::new(vec![1]);
        stack.current_mut().push(2);
        stack.push(vec![9]);
        stack.back();
        assert_eq!(*stack.current(), vec![1, 2]);
    }
}
