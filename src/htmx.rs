use anyhow::{anyhow, Error};
use axum::http::HeaderMap;
use std::str::FromStr;
use url::Url;

#[derive(Debug,Default,Clone,PartialEq,Eq,PartialOrd,Ord)]
pub struct HtmxContext {
    pub(crate) is_boost: bool,
    pub(crate) target: Option<String>,
    pub(crate) trigger: Option<String>,
    pub(crate) trigger_name: Option<String>,
    pub(crate) current_url: Option<Url>,
}

impl TryFrom<&HeaderMap> for HtmxContext {
    type Error = Error;

    /// Capture the [HtmxContext] from the request headers. Although this is a "try" it should
    /// never fail in reality if coming from a well-behaved client. This should only fail if
    /// the client is badly behaved or someone is manually injecting headers. Then they get
    /// what they deserve (undefined client side behavior)
    fn try_from(value: &HeaderMap) -> Result<Self, Self::Error> {
        if value.get("HX-Request").is_some_and(|x| x.eq("true")) {
            let mut out = HtmxContext{
                is_boost: value.get("HX-Boosted").is_some_and(|x| x.eq("true")),
                ..HtmxContext::default()
            };
            if let Some(r) = value.get("HX-Target") {
                out.target = Some(r.to_str()?.to_string());
            }
            if let Some(r) = value.get("HX-Trigger") {
                out.trigger = Some(r.to_str()?.to_string());
            }
            if let Some(r) = value.get("HX-Trigger-Name") {
                out.trigger_name = Some(r.to_str()?.to_string());
            }
            if let Some(r) = value.get("HX-Current-URL") {
                out.current_url = Some(Url::from_str(r.to_str()?)?);
            }
            Ok(out)
        } else {
            Err(anyhow!("HX-Request header is missing"))?
        }
    }
}