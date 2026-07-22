use std::collections::HashMap;
use std::sync::RwLock;

/// A skill is a named prompt template that the agent can look up and apply.
pub struct Skill {
    pub name: String,
    pub description: String,
    pub prompt: String,
}

/// Thread-safe registry of skills.
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, Skill>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new skill.
    pub fn add(&self, name: String, description: String, prompt: String) {
        let mut skills = self.skills.write().unwrap();
        tracing::info!("Registered skill: {}", name);
        skills.insert(
            name.clone(),
            Skill {
                name,
                description,
                prompt,
            },
        );
    }

    /// List all skills as "name: description" lines.
    pub fn list(&self) -> String {
        let skills = self.skills.read().unwrap();
        if skills.is_empty() {
            return "No skills registered.".to_string();
        }
        let mut lines: Vec<String> = skills
            .values()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect();
        lines.sort();
        lines.join("\n")
    }

    /// Get a skill's full prompt by name.
    pub fn get(&self, name: &str) -> Option<String> {
        let skills = self.skills.read().unwrap();
        skills.get(name).map(|s| s.prompt.clone())
    }

    /// Build a catalog string for injection into the backend thread's developer
    /// instructions. Each skill's full prompt is inlined (there is no lookup tool
    /// over ACP — the backend gets everything up front). Returns None if no skills
    /// are registered.
    pub fn catalog(&self) -> Option<String> {
        let skills = self.skills.read().unwrap();
        if skills.is_empty() {
            return None;
        }
        let mut entries: Vec<&Skill> = skills.values().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out =
            String::from("Available skills — apply the relevant one's instructions when it fits the request:\n");
        for s in entries {
            out.push_str(&format!("\n## {} — {}\n{}\n", s.name, s.description, s.prompt));
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_registry() {
        let registry = SkillRegistry::new();
        assert_eq!(registry.list(), "No skills registered.");
        assert!(registry.catalog().is_none());

        registry.add(
            "test-skill".to_string(),
            "A test skill".to_string(),
            "Do the test thing.".to_string(),
        );

        assert!(registry.list().contains("test-skill"));
        assert!(registry.list().contains("A test skill"));
        assert_eq!(registry.get("test-skill"), Some("Do the test thing.".to_string()));
        assert_eq!(registry.get("nonexistent"), None);
    }

    #[test]
    fn catalog_inlines_names_descriptions_and_prompts() {
        let registry = SkillRegistry::new();
        registry.add(
            "greeting".to_string(),
            "Greet the user".to_string(),
            "Say hello warmly.".to_string(),
        );
        let catalog = registry.catalog().unwrap();
        assert!(catalog.contains("greeting"));
        assert!(catalog.contains("Greet the user"));
        assert!(catalog.contains("Say hello warmly."));
    }
}
