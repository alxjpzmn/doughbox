use anyhow::bail;
use clap::ValueEnum;

pub const MAX_WORKERS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Feed {
    Xetra,
    Tradegate,
    Frankfurt,
    Cboe,
    Euronext,
}

impl Feed {
    pub const ALL: [Self; 5] = [
        Self::Xetra,
        Self::Tradegate,
        Self::Frankfurt,
        Self::Cboe,
        Self::Euronext,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Xetra => "DETR-posttrade",
            Self::Tradegate => "DGAT-posttrade",
            Self::Frankfurt => "DFRA-posttrade",
            Self::Cboe => "CBOE-posttrade",
            Self::Euronext => "EURONEXT-posttrade",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::Xetra | Self::Tradegate | Self::Frankfurt => "deutsche_boerse",
            Self::Cboe => "cboe",
            Self::Euronext => "euronext",
        }
    }

    pub fn from_key(key: &str) -> anyhow::Result<Self> {
        match key {
            "xetra" | "DETR-posttrade" => Ok(Self::Xetra),
            "tradegate" | "DGAT-posttrade" => Ok(Self::Tradegate),
            "frankfurt" | "DFRA-posttrade" => Ok(Self::Frankfurt),
            "cboe" | "CBOE-posttrade" => Ok(Self::Cboe),
            "euronext" | "EURONEXT-posttrade" => Ok(Self::Euronext),
            _ => bail!(
                "Unsupported price feed '{key}'; use xetra, tradegate, frankfurt, cboe or euronext"
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PriceConfig {
    pub enabled: bool,
    pub interval_minutes: u32,
    pub workers: usize,
    pub feeds: Vec<Feed>,
}

impl Default for PriceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 30,
            workers: 2,
            feeds: Feed::ALL.to_vec(),
        }
    }
}
