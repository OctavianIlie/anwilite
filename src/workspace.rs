pub const WORKSPACE_COUNT: usize = 10;

pub struct Workspaces {
    current: usize,
}

impl Workspaces {
    pub fn new() -> Self {
        Self { current: 0 }
    }

    pub fn current_number(&self) -> usize {
        self.current + 1
    }

    pub fn switch_to(&mut self, number: usize) -> bool {
        if !(1..=WORKSPACE_COUNT).contains(&number) {
            return false;
        }

        self.current = number - 1;
        true
    }
}

impl Default for Workspaces {
    fn default() -> Self {
        Self::new()
    }
}
