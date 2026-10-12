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

    pub fn lowered_volume(&self, current: f32) -> f32 {
        current.min(f32::from(self.percent) / 100.0)
    }
}

pub fn unchanged(current: f32, applied: f32) -> bool {
    (current - applied).abs() < 0.0001
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn never_raises_volume_when_lowering_and_respects_manual_changes() {
        let settings = Settings::default();
        assert!(unchanged(settings.lowered_volume(0.8), 0.4));
        assert!(unchanged(settings.lowered_volume(0.2), 0.2));
        assert!(!unchanged(0.25, 0.4));
        assert!(unchanged(0.400001, 0.4));
    }
}
