//! LAN discovery for the M7 UI.
//!
//! Receivers advertise themselves two ways and both carry the same fields, so
//! the host UI treats them identically:
//!
//! * mDNS service `_lowlat._udp.local.` (preferred; handled by `mdns-sd` in the
//!   apps, the constants live here),
//! * a UDP broadcast beacon on [`BEACON_PORT`] sent every two seconds
//!   (fallback for networks where multicast is blocked).
//!
//! The beacon is a single line of plain text:
//!
//! ```text
//! LOWLAT1|id|name|platform|width|height|orientation|video_port|pair_port
//! ```
//!
//! Free-form fields are sanitised so they can never contain the separator.

/// UDP port the fallback broadcast beacon is sent to.
pub const BEACON_PORT: u16 = 9200;
/// UDP port a receiver listens on for pairing messages.
pub const PAIR_PORT: u16 = 9201;
/// mDNS service type receivers register and hosts browse.
pub const MDNS_SERVICE: &str = "_lowlat._udp.local.";
/// First field of a beacon line; lets us ignore unrelated broadcast traffic.
pub const BEACON_MAGIC: &str = "LOWLAT1";

/// One receiver as advertised on the LAN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beacon {
    /// Stable per-installation id (used as the pairing identity).
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// `windows`, `linux`, ...
    pub platform: String,
    /// Advertised screen width in pixels.
    pub width: u32,
    /// Advertised screen height in pixels.
    pub height: u32,
    /// Natural orientation in degrees: 0, 90, 180, 270.
    pub orientation: u16,
    /// UDP port where the receiver accepts video.
    pub video_port: u16,
    /// UDP port where the receiver accepts pairing requests.
    pub pair_port: u16,
}

impl Beacon {
    /// Render the beacon as the single-line wire format.
    pub fn encode(&self) -> String {
        format!(
            "{BEACON_MAGIC}|{}|{}|{}|{}|{}|{}|{}|{}",
            sanitize(&self.id),
            sanitize(&self.name),
            sanitize(&self.platform),
            self.width,
            self.height,
            self.orientation,
            self.video_port,
            self.pair_port
        )
    }

    /// Parse a beacon line. Returns `None` for anything that is not a
    /// well-formed lowlat beacon.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('|');
        if parts.next()? != BEACON_MAGIC {
            return None;
        }
        let id = parts.next()?.to_string();
        let name = parts.next()?.to_string();
        let platform = parts.next()?.to_string();
        let width = parts.next()?.parse().ok()?;
        let height = parts.next()?.parse().ok()?;
        let orientation = parts.next()?.parse().ok()?;
        let video_port = parts.next()?.parse().ok()?;
        let pair_port = parts.next()?.parse().ok()?;
        // Reject trailing junk instead of silently ignoring it.
        if parts.next().is_some() {
            return None;
        }
        if id.is_empty() || name.is_empty() {
            return None;
        }
        Some(Self {
            id,
            name,
            platform,
            width,
            height,
            orientation,
            video_port,
            pair_port,
        })
    }
}

/// Keep `|` and control characters out of a field so the line stays parseable.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '|' || c.is_control() { '_' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Beacon {
        Beacon {
            id: "recv-01".into(),
            name: "Kitchen display".into(),
            platform: "linux".into(),
            width: 1280,
            height: 720,
            orientation: 90,
            video_port: 9100,
            pair_port: PAIR_PORT,
        }
    }

    #[test]
    fn beacon_roundtrip() {
        let beacon = sample();
        let text = beacon.encode();
        assert!(text.starts_with("LOWLAT1|"));
        assert_eq!(Beacon::parse(&text), Some(beacon));
    }

    #[test]
    fn beacon_rejects_wrong_magic() {
        let text = sample().encode().replace("LOWLAT1", "OTHER99");
        assert_eq!(Beacon::parse(&text), None);
    }

    #[test]
    fn beacon_rejects_bad_numbers() {
        let text = sample().encode().replace("|1280|", "|wide|");
        assert_eq!(Beacon::parse(&text), None);
    }

    #[test]
    fn beacon_rejects_missing_and_extra_fields() {
        assert_eq!(Beacon::parse("LOWLAT1|only|a|few"), None);
        let mut text = sample().encode();
        text.push_str("|extra");
        assert_eq!(Beacon::parse(&text), None);
    }

    #[test]
    fn beacon_sanitises_separators() {
        let mut beacon = sample();
        beacon.name = "bad|name\nwith controls".into();
        let text = beacon.encode();
        let parsed = Beacon::parse(&text).expect("still parses after sanitising");
        assert_eq!(parsed.name, "bad_name_with controls");
    }

    #[test]
    fn beacon_rejects_empty_identity() {
        let mut beacon = sample();
        beacon.id = String::new();
        assert_eq!(Beacon::parse(&beacon.encode()), None);
    }
}
