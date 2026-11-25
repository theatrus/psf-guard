use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// What an image's samples stand for.
///
/// This is about meaning, not numeric scale. `seiza_stretch::SampleDomain`
/// describes the scale of linear samples (unit or physical) and is a
/// separate question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Domain {
    /// Samples are proportional to light. Calibration, background removal,
    /// deconvolution and color calibration work here.
    Linear,
    /// Samples have been through a display stretch. Curves, saturation and
    /// local contrast work here.
    Display,
}

impl fmt::Display for Domain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Domain::Linear => "linear",
            Domain::Display => "display",
        })
    }
}

/// A word that names no [`Domain`].
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{0:?} is not a sample domain; use \"linear\" or \"display\"")]
pub struct ParseDomainError(String);

impl FromStr for Domain {
    type Err = ParseDomainError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        match word {
            "linear" => Ok(Domain::Linear),
            "display" => Ok(Domain::Display),
            other => Err(ParseDomainError(other.to_string())),
        }
    }
}

/// The domains an operation works in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Accepts {
    /// Linear samples only.
    Linear,
    /// Display-referred samples only.
    Display,
    /// Either. An operation that takes either should name the domain it
    /// was given in what it reports.
    Either,
}

impl fmt::Display for Accepts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Accepts::Linear => "linear",
            Accepts::Display => "display",
            Accepts::Either => "either",
        })
    }
}

/// A word that names no [`Accepts`].
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{0:?} does not say what an operation accepts; use \"linear\", \"display\" or \"either\"")]
pub struct ParseAcceptsError(String);

impl FromStr for Accepts {
    type Err = ParseAcceptsError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        match word {
            "linear" => Ok(Accepts::Linear),
            "display" => Ok(Accepts::Display),
            "either" => Ok(Accepts::Either),
            other => Err(ParseAcceptsError(other.to_string())),
        }
    }
}

/// An operation was given samples in a domain it does not work in.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{operation} works on {expected} samples and was given {given} samples")]
pub struct DomainError {
    /// The operation that refused.
    pub operation: String,
    /// The domain the operation works in.
    pub expected: Domain,
    /// The domain of the samples it was given.
    pub given: Domain,
}

impl Accepts {
    /// Checks samples in the `given` domain against what `operation` works
    /// in. An operation calls this before it touches a sample, so it never
    /// guesses at what it was handed.
    pub fn check(self, operation: &str, given: Domain) -> Result<(), DomainError> {
        let expected = match self {
            Accepts::Either => return Ok(()),
            Accepts::Linear => Domain::Linear,
            Accepts::Display => Domain::Display,
        };
        if given == expected {
            Ok(())
        } else {
            Err(DomainError {
                operation: operation.to_string(),
                expected,
                given,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_is_spelled_as_a_word() {
        for domain in [Domain::Linear, Domain::Display] {
            assert_eq!(domain.to_string().parse::<Domain>(), Ok(domain));
            let json = serde_json::to_string(&domain).unwrap();
            assert_eq!(json, format!("\"{domain}\""));
        }
        assert!("stretched".parse::<Domain>().is_err());
        for accepts in [Accepts::Linear, Accepts::Display, Accepts::Either] {
            assert_eq!(accepts.to_string().parse::<Accepts>(), Ok(accepts));
            let json = serde_json::to_string(&accepts).unwrap();
            assert_eq!(json, format!("\"{accepts}\""));
        }
        assert!("both".parse::<Accepts>().is_err());
    }

    #[test]
    fn the_wrong_domain_is_refused_by_name() {
        assert_eq!(
            Accepts::Linear.check("deconvolution", Domain::Linear),
            Ok(())
        );
        assert_eq!(Accepts::Either.check("SCNR", Domain::Display), Ok(()));
        let refusal = Accepts::Display
            .check("curves", Domain::Linear)
            .unwrap_err();
        assert_eq!(refusal.expected, Domain::Display);
        assert_eq!(refusal.given, Domain::Linear);
        assert_eq!(
            refusal.to_string(),
            "curves works on display samples and was given linear samples"
        );
    }
}
