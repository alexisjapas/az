use std::env;

use thiserror::Error;

pub const ENV_SESSION_MODE: &str = "AZ_SESSION_MODE";

/// Mode d'une session vis-à-vis du monde extérieur.
///
/// - `Private` : aucun accès externe, le LLM (local) peut voir toutes les
///   entrées y compris `sensitivity = true`.
/// - `Connected` : outils externes autorisés. Les entrées `sensitivity = true`
///   sont **exclues** techniquement avant tout passage par le LLM.
///
/// Le filtrage est appliqué en base, pas par convention au niveau du modèle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Private,
    Connected,
}

/// Filtre de lecture appliqué aux méthodes qui alimentent un LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadFilter {
    #[default]
    All,
    ExcludeSensitive,
}

#[derive(Debug, Error)]
pub enum SessionParseError {
    #[error("mode session inconnu: '{0}' (attendu: private | connected)")]
    UnknownMode(String),
}

impl SessionMode {
    /// Mode par défaut quand rien n'est précisé.
    pub const DEFAULT: SessionMode = SessionMode::Private;

    pub fn read_filter(self) -> ReadFilter {
        match self {
            SessionMode::Private => ReadFilter::All,
            SessionMode::Connected => ReadFilter::ExcludeSensitive,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SessionMode::Private => "private",
            SessionMode::Connected => "connected",
        }
    }

    pub fn parse(s: &str) -> Result<Self, SessionParseError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "private" | "priv" | "p" => Ok(SessionMode::Private),
            "connected" | "conn" | "c" => Ok(SessionMode::Connected),
            other => Err(SessionParseError::UnknownMode(other.to_string())),
        }
    }

    /// CLI > env var > défaut.
    pub fn resolve(cli_arg: Option<&str>) -> Result<Self, SessionParseError> {
        if let Some(s) = cli_arg {
            return Self::parse(s);
        }
        if let Ok(s) = env::var(ENV_SESSION_MODE) {
            return Self::parse(&s);
        }
        Ok(Self::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_modes() {
        assert_eq!(SessionMode::parse("private").unwrap(), SessionMode::Private);
        assert_eq!(SessionMode::parse("Private").unwrap(), SessionMode::Private);
        assert_eq!(SessionMode::parse("p").unwrap(), SessionMode::Private);
        assert_eq!(
            SessionMode::parse("connected").unwrap(),
            SessionMode::Connected
        );
        assert_eq!(SessionMode::parse("c").unwrap(), SessionMode::Connected);
        assert!(SessionMode::parse("bogus").is_err());
    }

    #[test]
    fn read_filter_mapping() {
        assert_eq!(SessionMode::Private.read_filter(), ReadFilter::All);
        assert_eq!(
            SessionMode::Connected.read_filter(),
            ReadFilter::ExcludeSensitive
        );
    }

    #[test]
    fn resolve_cli_overrides_env() {
        // Petite ruse : on ne peut pas garantir l'isolation des env vars en parallèle,
        // donc on teste que cli_arg = Some prend toujours le pas.
        unsafe {
            env::set_var(ENV_SESSION_MODE, "connected");
        }
        let m = SessionMode::resolve(Some("private")).unwrap();
        assert_eq!(m, SessionMode::Private);
        unsafe {
            env::remove_var(ENV_SESSION_MODE);
        }
    }
}
