#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub percent: u8,
    pub executable: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            percent: 40,
            executable: "wardogs".into(),
        }
    }
}

impl Settings {
    pub fn parse(percent: &str, executable: &str) -> Result<Self, &'static str> {
        let percent = percent
            .trim()
            .parse::<u8>()
            .map_err(|_| "Enter a whole percentage from 0 to 100.")?;
        if percent > 100 {
            return Err("Enter a whole percentage from 0 to 100.");
        }
        let executable = executable.trim().to_ascii_lowercase();
        if executable.is_empty()
            || executable.len() > 100
            || executable
                .chars()
                .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        {
            return Err(
                "Enter a game executable name or a distinctive part of it, such as wardogs.",
            );
        }
        Ok(Self {
            percent,
            executable,
        })
    }

    pub fn decode(text: &str) -> Result<Self, &'static str> {
        let mut lines = text.lines();
        if lines.next() != Some("earplugs-v1") {
            return Err("Unrecognized settings format.");
        }
        let result = Self::parse(
            lines.next().ok_or("Missing percentage.")?,
            lines.next().ok_or("Missing executable.")?,
        )?;
        if lines.next().is_some() {
            return Err("Unexpected settings content.");
        }
        Ok(result)
    }

    pub fn encode(&self) -> String {
        format!("earplugs-v1\n{}\n{}\n", self.percent, self.executable)
    }

    pub fn matches(&self, executable: &str) -> bool {
        let name = executable.to_ascii_lowercase();
        if self.executable.ends_with(".exe") {
            name == self.executable
        } else {
            name.contains(&self.executable)
        }
    }

    pub fn target_volume(&self) -> f32 {
        f32::from(self.percent) / 100.0
    }
}

pub fn unchanged(current: f32, applied: f32) -> bool {
    (current - applied).abs() < 0.0001
}

#[derive(Default)]
pub struct F2Press {
    down: Vec<usize>,
}

impl F2Press {
    pub fn event(&mut self, device: usize, key: u16, down: bool) -> bool {
        if key != 0x71 {
            return false;
        }
        if down {
            if self.down.contains(&device) {
                return false;
            }
            self.down.push(device);
            true
        } else {
            self.remove(device);
            false
        }
    }

    pub fn remove(&mut self, device: usize) {
        self.down.retain(|held| *held != device);
    }

    pub fn reset(&mut self) {
        self.down.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_keyboards_do_not_release_each_others_f2() {
        let mut press = F2Press::default();
        let events = [(1, true), (2, true), (2, false), (1, true), (1, false)];
        let toggles: Vec<usize> = events
            .into_iter()
            .enumerate()
            .filter_map(|(index, (device, down))| press.event(device, 0x71, down).then_some(index))
            .collect();
        assert_eq!(toggles, [0, 1], "Each keyboard press must toggle once; another keyboard's release must not rearm a held key");
    }
    #[test]
    fn held_f2_ignores_repeat_and_modifier_changes_until_released() {
        let mut press = F2Press::default();
        let events = [
            (0x57, true),
            (0xa0, true),
            (0x71, true),
            (0x71, true),
            (0xa2, true),
            (0x71, true),
            (0xa0, false),
            (0x71, true),
            (0xa2, false),
            (0x71, true),
            (0x71, false),
            (0x71, true),
            (0x71, false),
            (0x57, false),
        ];
        let presses: Vec<usize> = events
            .into_iter()
            .enumerate()
            .filter_map(|(index, (key, down))| press.event(0, key, down).then_some(index))
            .collect();
        assert_eq!(presses, [2, 11], "Only fresh F2 presses may toggle");
    }
    #[test]
    fn removal_and_resume_clear_stale_keyboard_state() {
        let mut press = F2Press::default();
        assert!(press.event(1, 0x71, true));
        assert!(press.event(2, 0x71, true));
        press.remove(1);
        assert!(!press.event(2, 0x71, true));
        assert!(press.event(1, 0x71, true));
        press.reset();
        assert!(press.event(2, 0x71, true));
    }
    #[test]
    fn defaults_and_persistence() {
        let settings = Settings::default();
        assert_eq!(settings.percent, 40);
        assert_eq!(Settings::decode(&settings.encode()), Ok(settings));
    }
    #[test]
    fn validates_boundaries_and_corrupt_settings() {
        for percent in ["0", "40", "100", " 25 "] {
            assert!(Settings::parse(percent, "WarDogs").is_ok());
        }
        for percent in ["", "-1", "101", "256", "NaN", "40.5"] {
            assert!(Settings::parse(percent, "wardogs").is_err());
        }
        for executable in ["", "*", "C:\\game.exe", "dog\nother"] {
            assert!(Settings::parse("40", executable).is_err());
        }
        assert!(Settings::decode("earplugs-v1\n40\nwardogs\nextra").is_err());
        assert!(Settings::decode("wrong\n40\nwardogs").is_err());
    }
    #[test]
    fn finds_game_without_matching_other_audio() {
        let settings = Settings::default();
        assert!(settings.matches("WARdogs-Win64-Shipping.exe"));
        assert!(!settings.matches("Discord.exe"));
        let exact = Settings::parse("40", "WarDogs.exe").unwrap();
        assert!(exact.matches("wardogs.exe"));
        assert!(!exact.matches("other-wardogs.exe"));
    }
    #[test]
    fn exact_percentage_targets_and_manual_change_detection() {
        let settings = Settings::default();
        assert!(unchanged(settings.target_volume(), 0.4));
        for (percent, target) in [("0", 0.0), ("60", 0.6), ("80", 0.8), ("100", 1.0)] {
            assert!(unchanged(
                Settings::parse(percent, "wardogs").unwrap().target_volume(),
                target
            ));
        }
        assert!(!unchanged(0.25, 0.4));
        assert!(unchanged(0.400001, 0.4));
    }
}
