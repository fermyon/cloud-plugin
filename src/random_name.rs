use std::collections::HashSet;

use rand::seq::SliceRandom;

const ADJECTIVES: &str = include_str!("adjectives.txt");
const NOUNS: &str = include_str!("nouns.txt");

pub struct RandomNameGenerator {
    adjectives: Vec<&'static str>,
    nouns: Vec<&'static str>,
}

impl RandomNameGenerator {
    pub fn new() -> Self {
        let adjectives = ADJECTIVES.split('\n').collect();
        let nouns = NOUNS.split('\n').collect();
        Self { adjectives, nouns }
    }

    pub fn generate(&self) -> String {
        let mut rng = rand::thread_rng();
        let adjective = self.adjectives.choose(&mut rng).unwrap();
        let noun = self.nouns.choose(&mut rng).unwrap();
        format!("{adjective}-{noun}")
    }

    pub fn generate_unique(&self, existing: HashSet<&str>, max_attempts: usize) -> Option<String> {
        let mut i = 0;
        while i < max_attempts {
            let name = self.generate();
            if !existing.contains(name.as_str()) {
                return Some(name);
            }
            i += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_produce_name() {
        let generator = RandomNameGenerator::new();
        assert!(generator.generate_unique(Default::default(), 10).is_some());
    }
}
