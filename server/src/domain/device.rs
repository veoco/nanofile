//! The device a request came from, as far as the server can tell.
//!
//! A login reports platform, `device_id`, name and version, and the sync protocol
//! reports the same id as `client_id`; credentials are grouped by those values. A
//! session, an API key or an older client that reported nothing carries no
//! identity: its token is minted unattributed and claimed on first sync.

/// A device identity the server learned from the authenticated request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    /// Display-only, e.g. `windows`. Not stored on a sync token.
    pub platform: Option<String>,
    /// The stable client id: the login `device_id` and the sync `client_id`.
    pub device_id: String,
    pub device_name: Option<String>,
    pub client_version: Option<String>,
}

impl DeviceIdentity {
    /// Build one from the fields a client may have reported. An empty or missing
    /// `device_id` is not an identity — two devices would be indistinguishable —
    /// so that case yields `None`.
    pub fn new(
        platform: Option<String>,
        device_id: Option<String>,
        device_name: Option<String>,
        client_version: Option<String>,
    ) -> Option<Self> {
        let device_id = device_id.filter(|id| !id.is_empty())?;
        Some(Self {
            platform: platform.filter(|value| !value.is_empty()),
            device_id,
            device_name: device_name.filter(|value| !value.is_empty()),
            client_version: client_version.filter(|value| !value.is_empty()),
        })
    }

    /// The subset a sync token records as its peer info.
    pub fn peer(&self) -> PeerStamp {
        PeerStamp {
            id: self.device_id.clone(),
            name: self.device_name.clone(),
            client_version: self.client_version.clone(),
        }
    }
}

/// The device fields a sync token row stores; `platform` is a property of the
/// session, not of the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStamp {
    pub id: String,
    pub name: Option<String>,
    pub client_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_device_id_is_not_an_identity() {
        assert!(DeviceIdentity::new(None, None, Some("laptop".into()), None).is_none());
        assert!(DeviceIdentity::new(None, Some(String::new()), None, None).is_none());

        let device = DeviceIdentity::new(
            Some("linux".into()),
            Some("dev-a".into()),
            Some("laptop".into()),
            Some("3.0.4".into()),
        )
        .expect("a reported device id is an identity");
        assert_eq!(device.platform.as_deref(), Some("linux"));
        assert_eq!(
            device.peer(),
            PeerStamp {
                id: "dev-a".into(),
                name: Some("laptop".into()),
                client_version: Some("3.0.4".into()),
            }
        );
    }

    #[test]
    fn blank_optional_fields_are_dropped() {
        let device =
            DeviceIdentity::new(None, Some("dev-a".into()), Some(String::new()), None).unwrap();
        assert_eq!(device.platform, None);
        assert_eq!(device.peer().name, None);
        assert_eq!(device.peer().client_version, None);
    }
}
