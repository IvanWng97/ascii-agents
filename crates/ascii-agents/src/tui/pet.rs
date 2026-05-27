#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PetKind {
    Cat,
    Dog,
}

impl PetKind {
    pub const ALL: &'static [PetKind] = &[PetKind::Cat, PetKind::Dog];

    pub fn config_name(self) -> &'static str {
        match self {
            PetKind::Cat => "cat",
            PetKind::Dog => "dog",
        }
    }

    pub fn from_config_name(s: &str) -> Option<Self> {
        match s {
            "cat" => Some(PetKind::Cat),
            "dog" => Some(PetKind::Dog),
            _ => None,
        }
    }

    pub fn walk_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_walk",
            PetKind::Dog => "dog_walk",
        }
    }

    pub fn sit_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_sit",
            PetKind::Dog => "dog_sit",
        }
    }

    pub fn sleep_anim(self) -> &'static str {
        match self {
            PetKind::Cat => "cat_sleep",
            PetKind::Dog => "dog_sleep",
        }
    }

    pub fn sleeps_near_idle(self) -> bool {
        match self {
            PetKind::Cat => true,
            PetKind::Dog => false,
        }
    }

    pub fn hitbox(self, anim_name: &str) -> (u16, u16) {
        match (self, anim_name) {
            (PetKind::Cat, "cat_walk") => (8, 6),
            (PetKind::Cat, "cat_sit") => (6, 6),
            (PetKind::Cat, "cat_sleep") => (6, 4),
            (PetKind::Dog, "dog_walk") => (8, 6),
            (PetKind::Dog, "dog_sit") => (6, 6),
            (PetKind::Dog, "dog_sleep") => (6, 4),
            _ => (6, 6),
        }
    }
}

pub fn select_pet_for_floor(floor_seed: u64, enabled_pets: &[PetKind]) -> Option<PetKind> {
    if enabled_pets.is_empty() {
        return None;
    }
    Some(enabled_pets[(floor_seed as usize) % enabled_pets.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_name_roundtrip() {
        for &kind in PetKind::ALL {
            assert_eq!(PetKind::from_config_name(kind.config_name()), Some(kind));
        }
    }

    #[test]
    fn from_config_name_unknown_returns_none() {
        assert_eq!(PetKind::from_config_name("hamster"), None);
    }

    #[test]
    fn select_pet_empty_returns_none() {
        assert_eq!(select_pet_for_floor(42, &[]), None);
    }

    #[test]
    fn select_pet_single_always_returns_it() {
        assert_eq!(select_pet_for_floor(0, &[PetKind::Dog]), Some(PetKind::Dog));
        assert_eq!(
            select_pet_for_floor(99, &[PetKind::Dog]),
            Some(PetKind::Dog)
        );
    }

    #[test]
    fn select_pet_two_pets_alternates_by_seed() {
        let pets = vec![PetKind::Cat, PetKind::Dog];
        let floor0 = select_pet_for_floor(0, &pets);
        let floor1 = select_pet_for_floor(1, &pets);
        assert_ne!(floor0, floor1);
    }

    #[test]
    fn anim_names_match_kind() {
        assert!(PetKind::Cat.walk_anim().starts_with("cat_"));
        assert!(PetKind::Dog.walk_anim().starts_with("dog_"));
    }
}
