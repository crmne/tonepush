//! Turning raw parameter values into the text HX Edit shows.
//!
//! The device deals in native units - `0.78`, `-0.1`, `1.0`. HX Edit shows
//! "78%", "-0.1 dB", "Limit". The mapping lives in `HelixControls.json` as a
//! small formatting language: an optional scale, then either a printf pattern,
//! a list of labels for a menu, or a set of ranges each with its own pattern.

use std::collections::HashSet;

use serde::Deserialize;

use crate::{Catalog, Kind, Param};

/// How one family of parameters is displayed.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Display {
    /// Defer to another entry. Roughly a quarter of the table is aliases.
    alias: Option<String>,
    /// Multiply before display: percentages are stored 0..1 and shown 0..100.
    #[serde(rename = "dspToDisplayScale")]
    scale: Option<f32>,
    #[serde(rename = "dspToDisplayIntegerOffset")]
    offset: Option<f32>,
    format: Option<Pattern>,
    #[serde(rename = "formatUnits")]
    format_units: Option<String>,
}

/// The `format` field wears three different hats depending on the parameter.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Pattern {
    /// A printf pattern, e.g. `%+.1f`.
    Printf(String),
    /// Menu labels, indexed by value.
    Labels(Vec<String>),
    /// Per-range patterns, for parameters that read differently at each end.
    Ranges(Vec<Range>),
}

#[derive(Debug, Clone, Deserialize)]
struct Range {
    #[serde(rename = "lowerBound")]
    lower: f32,
    #[serde(rename = "upperBound")]
    upper: f32,
    #[serde(rename = "formatUnits")]
    format_units: Option<String>,
    format: Option<String>,
    #[serde(rename = "unitsMultiplier")]
    multiplier: Option<f32>,
}

impl Display {
    /// Follow aliases without trusting the external catalog to be acyclic.
    ///
    /// A missing target keeps the last usable entry, matching the old
    /// graceful fallback. A cycle keeps the entry that would traverse the
    /// repeated edge, so its own format remains usable instead of recursing
    /// until the process exhausts its stack.
    fn resolved<'a>(&'a self, catalog: &'a Catalog) -> &'a Display {
        let mut display = self;
        let mut followed = HashSet::new();
        while let Some(alias) = display.alias.as_deref() {
            if !followed.insert(alias) {
                break;
            }
            let Some(target) = catalog.display(alias) else {
                break;
            };
            display = target;
        }
        display
    }

    pub(crate) fn render(&self, value: f32, catalog: &Catalog) -> String {
        let display = self.resolved(catalog);

        let scaled = value * display.scale.unwrap_or(1.0) + display.offset.unwrap_or(0.0);

        match &display.format {
            Some(Pattern::Labels(labels)) => labels
                .get(scaled.round().max(0.0) as usize)
                .cloned()
                .unwrap_or_else(|| trim(scaled)),
            Some(Pattern::Ranges(ranges)) => ranges
                .iter()
                .find(|r| scaled >= r.lower && scaled < r.upper)
                .map(|r| {
                    let v = scaled * r.multiplier.unwrap_or(1.0);
                    let pattern = r.format_units.as_ref().or(r.format.as_ref());
                    pattern.map_or_else(|| trim(v), |p| printf(p, v))
                })
                .unwrap_or_else(|| trim(scaled)),
            Some(Pattern::Printf(p)) => printf(display.format_units.as_ref().unwrap_or(p), scaled),
            None => display
                .format_units
                .as_ref()
                .map_or_else(|| trim(scaled), |p| printf(p, scaled)),
        }
    }
}

impl Display {
    /// Turn a value the user typed back into the units the device wants.
    ///
    /// The exact inverse of [`render`](Self::render) for the common cases:
    /// undo the scale and offset, or match a menu label. A ranged format's unit
    /// identifies its multiplier: `1.2 kHz`, for example, reverses the
    /// frequency range's `0.001` multiplier and becomes `1200`.
    pub(crate) fn to_native(&self, shown: f32, unit: &str, catalog: &Catalog) -> f32 {
        let display = self.resolved(catalog);
        let scaled = if unit.is_empty() {
            shown
        } else {
            match &display.format {
                Some(Pattern::Ranges(ranges)) => ranges
                    .iter()
                    .find(|range| range.has_unit(unit))
                    .and_then(|range| range.multiplier)
                    .filter(|multiplier| *multiplier != 0.0)
                    .map_or(shown, |multiplier| shown / multiplier),
                _ => shown,
            }
        };
        (scaled - display.offset.unwrap_or(0.0)) / display.scale.unwrap_or(1.0)
    }

    /// The index of a menu label, for parameters displayed as a word.
    pub(crate) fn label_index(&self, text: &str, catalog: &Catalog) -> Option<f32> {
        match &self.resolved(catalog).format {
            Some(Pattern::Labels(labels)) => labels
                .iter()
                .position(|l| l.eq_ignore_ascii_case(text))
                .map(|i| i as f32),
            _ => None,
        }
    }
}

impl Range {
    fn has_unit(&self, wanted: &str) -> bool {
        self.format_units
            .as_deref()
            .or(self.format.as_deref())
            .and_then(|pattern| {
                let percent = pattern.find('%')?;
                let rest = &pattern[percent + 1..];
                let conversion = rest.find(['f', 'd'])?;
                Some(rest[conversion + 1..].trim())
            })
            .is_some_and(|unit| unit.eq_ignore_ascii_case(wanted))
    }
}

impl Display {
    /// The menu this parameter offers, if it is one you pick from a list.
    pub(crate) fn choices<'a>(&'a self, catalog: &'a Catalog) -> Option<&'a [String]> {
        match &self.resolved(catalog).format {
            Some(Pattern::Labels(labels)) => Some(labels),
            _ => None,
        }
    }
}

/// Fallback for parameters with no display entry at all.
pub(crate) fn plain(param: &Param, value: f32) -> String {
    match param.kind {
        Kind::Switch => if value >= 0.5 { "On" } else { "Off" }.to_string(),
        Kind::Enum => format!("{}", value.round() as i64),
        _ => trim(value),
    }
}

/// A number without trailing noise: `1` not `1.0000`, `0.78` not `0.7800001`.
fn trim(v: f32) -> String {
    if (v - v.round()).abs() < 1e-4 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The sliver of printf the catalog actually uses: `%[+][0N][.N]f`, `%d`, `%%`.
///
/// Writing this out is less work than taking on a formatting dependency, and
/// the catalog only ever needs one substitution per pattern.
fn printf(pattern: &str, value: f32) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }

        let plus = chars.next_if_eq(&'+').is_some();
        let zero_pad = chars.peek() == Some(&'0');
        let mut width = 0usize;
        while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
            chars.next();
            width = width.saturating_mul(10).saturating_add(digit as usize);
        }
        let precision = if chars.next_if_eq(&'.').is_some() {
            let mut digits = 0usize;
            let mut value = 0usize;
            while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
                chars.next();
                digits += 1;
                value = value.saturating_mul(10).saturating_add(digit as usize);
            }
            (digits != 0).then_some(value)
        } else {
            None
        };

        match chars.next() {
            Some('f') => {
                let precision = precision.unwrap_or(0);
                let formatted = format!("{:.precision$}", value.abs());
                let sign = if value.is_sign_negative() {
                    Some('-')
                } else if plus {
                    Some('+')
                } else {
                    None
                };
                push_padded(&mut out, &formatted, sign, width, zero_pad);
            }
            Some('d') => {
                let rounded = value.round() as i64;
                let digits = rounded.unsigned_abs().to_string();
                let sign = if rounded < 0 {
                    Some('-')
                } else if plus {
                    Some('+')
                } else {
                    None
                };
                push_padded(&mut out, &digits, sign, width, zero_pad);
            }
            // Anything else is a pattern we have not seen; show the number
            // rather than dropping it.
            Some(other) => {
                out.push_str(&trim(value));
                out.push(other);
            }
            None => out.push_str(&trim(value)),
        }
    }
    out
}

fn push_padded(out: &mut String, value: &str, sign: Option<char>, width: usize, zero_pad: bool) {
    if zero_pad {
        if let Some(sign) = sign {
            out.push(sign);
        }
    }
    for _ in value.len() + usize::from(sign.is_some())..width {
        out.push(if zero_pad { '0' } else { ' ' });
    }
    if !zero_pad {
        if let Some(sign) = sign {
            out.push(sign);
        }
    }
    out.push_str(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printf_handles_the_patterns_the_catalog_uses() {
        assert_eq!(printf("%.0f %%", 78.0), "78 %");
        assert_eq!(printf("%+.1f dB", -0.1), "-0.1 dB");
        assert_eq!(printf("%+.1f dB", 3.0), "+3.0 dB");
        assert_eq!(printf("%.1f Hz", 4.25), "4.2 Hz");
        assert_eq!(printf("%.2f \"", 0.5), "0.50 \"");
        assert_eq!(printf("%03d", 5.0), "005");
        assert_eq!(printf("%03d", -5.0), "-05");
        assert_eq!(printf("%+03d", 5.0), "+05");
    }

    #[test]
    fn trims_pointless_decimals() {
        assert_eq!(trim(1.0), "1");
        assert_eq!(trim(0.78), "0.78");
        assert_eq!(trim(-54.0), "-54");
    }

    #[test]
    fn formats_real_parameters_like_hx_edit() {
        let Some(catalog) = crate::tests::catalog() else {
            return;
        };
        let comp = catalog.model("HD2_CompressorLAStudioComp").unwrap();

        let mix = comp.params.iter().find(|p| p.name == "Mix").unwrap();
        assert_eq!(catalog.format(mix, 1.0), "100 %");

        let level = comp.params.iter().find(|p| p.name == "Level").unwrap();
        assert_eq!(catalog.format(level, 0.0), "+0.0 dB");

        // A switch renders as its label, not as a number.
        let ty = comp.params.iter().find(|p| p.name == "Type").unwrap();
        assert_eq!(catalog.format(ty, 1.0), "Limit");
        assert_eq!(catalog.format(ty, 0.0), "Compress");

        // `valueType: integer` does not necessarily mean a named menu. Pitch
        // Wham's endpoints are stepped signed numbers; `choices` is the
        // distinction the GUI must use before drawing a ComboBox.
        let wham = catalog.model("HD2_PitchPitchWham").unwrap();
        let heel = wham.params.iter().find(|p| p.name == "Heel Pitch").unwrap();
        assert_eq!(heel.kind, Kind::Enum);
        assert!(catalog.choices(heel).is_none());
        assert_eq!(catalog.format(heel, -12.0), "-12");
        assert_eq!(catalog.format(heel, 12.0), "+12");
    }
}
