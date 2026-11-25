use chrono::{DateTime, TimeDelta, Utc};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Restart,
    Bugcheck,
    Crash,
    Hang,
    GraphicsReset,
    Update,
    PlannedRestart,
    ServiceRecovery,
    ServiceStopped,
    HardwareCorrected,
    HardwareFatal,
    Wake,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(crate) struct HelpArticle {
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
}

const RESTART_HELP: HelpArticle = HelpArticle {
    id: "restart",
    label: "Microsoft help for unexpected restarts",
    url: "https://support.microsoft.com/en-us/windows/experience/performance-optimization/troubleshooting-windows-unexpected-restarts-and-stop-code-errors",
};
const APP_HELP: HelpArticle = HelpArticle {
    id: "app",
    label: "Microsoft help for repairing apps",
    url: "https://support.microsoft.com/en-gb/windows/apps/repair-apps-and-programs-in-windows",
};
const DISPLAY_HELP: HelpArticle = HelpArticle {
    id: "display",
    label: "Microsoft help for blank or stuck screens",
    url: "https://support.microsoft.com/en-us/windows/hardware/display-graphics/troubleshooting-blank-screens-in-windows",
};
const UPDATE_HELP: HelpArticle = HelpArticle {
    id: "update",
    label: "Microsoft help for Windows Update history",
    url: "https://support.microsoft.com/en-us/windows/deployment/updates-lifecycle/windows-update-faq",
};

#[cfg(windows)]
pub(crate) fn official_help(id: &str) -> Option<HelpArticle> {
    const DESTINATION_HELP: HelpArticle = HelpArticle {
        id: "windows-destinations",
        label: "Microsoft’s Windows endpoint guide",
        url: "https://learn.microsoft.com/en-us/windows/privacy/manage-windows-11-endpoints",
    };
    [
        RESTART_HELP,
        APP_HELP,
        DISPLAY_HELP,
        UPDATE_HELP,
        DESTINATION_HELP,
    ]
    .into_iter()
    .find(|article| article.id == id)
}

impl Kind {
    pub const ALL: [Self; 12] = [
        Self::Restart,
        Self::Bugcheck,
        Self::Crash,
        Self::Hang,
        Self::GraphicsReset,
        Self::Update,
        Self::PlannedRestart,
        Self::ServiceRecovery,
        Self::ServiceStopped,
        Self::HardwareCorrected,
        Self::HardwareFatal,
        Self::Wake,
    ];

    pub fn is_incident(self) -> bool {
        !matches!(self, Self::Update | Self::PlannedRestart | Self::Wake)
    }

    pub fn from_event(provider: &str, id: u16) -> Option<Self> {
        match (provider, id) {
            ("Microsoft-Windows-Kernel-Power", 41) => Some(Self::Restart),
            ("Microsoft-Windows-WER-SystemErrorReporting", 1001) => Some(Self::Bugcheck),
            ("Application Error", 1000) => Some(Self::Crash),
            ("Application Hang", 1002) => Some(Self::Hang),
            ("Display", 4101) => Some(Self::GraphicsReset),
            ("Microsoft-Windows-WindowsUpdateClient", 19) => Some(Self::Update),
            ("User32" | "USER32", 1074) => Some(Self::PlannedRestart),
            ("Service Control Manager", 7031) => Some(Self::ServiceRecovery),
            ("Service Control Manager", 7034) => Some(Self::ServiceStopped),
            ("Microsoft-Windows-WHEA-Logger", 17 | 19) => Some(Self::HardwareCorrected),
            ("Microsoft-Windows-WHEA-Logger", 18) => Some(Self::HardwareFatal),
            ("Microsoft-Windows-Power-Troubleshooter", 1) => Some(Self::Wake),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Restart => "Unexpected restart",
            Self::Bugcheck => "Windows stop error",
            Self::Crash => "Application crashed",
            Self::Hang => "Application stopped responding",
            Self::GraphicsReset => "Graphics driver recovered",
            Self::Update => "Windows update installed",
            Self::PlannedRestart => "Shutdown or restart requested",
            Self::ServiceRecovery => "Windows service crashed with a recovery action",
            Self::ServiceStopped => "Windows service stopped unexpectedly",
            Self::HardwareCorrected => "Windows logged a corrected hardware error",
            Self::HardwareFatal => "Windows logged a fatal hardware error",
            Self::Wake => "Windows resumed from sleep",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::Restart => {
                "Windows started after an unclean shutdown. This record alone cannot tell us why."
            }
            Self::Bugcheck => {
                "Windows recorded a stop error. The timestamp is when Windows logged it, which can be after the restart."
            }
            Self::Crash => {
                "Windows logged an application error. This event records the crash. More evidence is needed to identify its cause."
            }
            Self::Hang => "Windows recorded an application that stopped responding.",
            Self::GraphicsReset => {
                "Windows recorded a graphics driver recovery. Check whether the timestamp lines up with what you saw."
            }
            Self::Update => {
                "Windows logged a successful update. Its timing gives context for the other events."
            }
            Self::PlannedRestart => "Windows logged a shutdown or restart request.",
            Self::ServiceRecovery => {
                "Windows logged an unexpected service exit and recorded a recovery action."
            }
            Self::ServiceStopped => "Windows logged an unexpected service exit.",
            Self::HardwareCorrected => {
                "Windows recorded a hardware error that was corrected. Repeated records can help narrow an investigation."
            }
            Self::HardwareFatal => {
                "Windows recorded a fatal hardware error. This record can help establish when the problem occurred."
            }
            Self::Wake => {
                "Windows recorded a return from sleep. The raw record may include a wake source."
            }
        }
    }

    pub(crate) fn next_step(self) -> &'static str {
        match self {
            Self::Restart => {
                "Look near the restart in the timeline for a stop error or hardware error."
            }
            Self::Bugcheck => "Save the stop code so you can compare it if Windows crashes again.",
            Self::Crash | Self::Hang => {
                "Check whether the same app crashes or freezes again while you do the same thing."
            }
            Self::GraphicsReset => {
                "Check whether the time matches when you saw your screen flicker, freeze or go blank."
            }
            Self::Update => {
                "Look up the update in Windows Update history if the problem started around this time."
            }
            Self::PlannedRestart => {
                "Check the record details for the app or Windows service that requested the restart."
            }
            Self::ServiceRecovery | Self::ServiceStopped => {
                "Check whether the same service stopped again around the time you noticed a problem."
            }
            Self::HardwareCorrected | Self::HardwareFatal => {
                "Check Crashes & events for other hardware errors around the same time."
            }
            Self::Wake => "Check whether the PC woke when you expected it to.",
        }
    }

    pub(crate) fn help(self) -> Option<HelpArticle> {
        match self {
            Self::Restart | Self::Bugcheck => Some(RESTART_HELP),
            Self::Crash | Self::Hang => Some(APP_HELP),
            Self::GraphicsReset => Some(DISPLAY_HELP),
            Self::Update => Some(UPDATE_HELP),
            _ => None,
        }
    }

    pub fn provider(self) -> &'static str {
        match self {
            Self::Restart => "Microsoft-Windows-Kernel-Power",
            Self::Bugcheck => "Microsoft-Windows-WER-SystemErrorReporting",
            Self::Crash => "Application Error",
            Self::Hang => "Application Hang",
            Self::GraphicsReset => "Display",
            Self::Update => "Microsoft-Windows-WindowsUpdateClient",
            Self::PlannedRestart => "User32",
            Self::ServiceRecovery | Self::ServiceStopped => "Service Control Manager",
            Self::HardwareCorrected | Self::HardwareFatal => "Microsoft-Windows-WHEA-Logger",
            Self::Wake => "Microsoft-Windows-Power-Troubleshooter",
        }
    }

    pub(crate) fn demo_id(self) -> u16 {
        match self {
            Self::Restart => 41,
            Self::Bugcheck => 1001,
            Self::Crash => 1000,
            Self::Hang => 1002,
            Self::GraphicsReset => 4101,
            Self::Update => 19,
            Self::PlannedRestart => 1074,
            Self::ServiceRecovery => 7031,
            Self::ServiceStopped => 7034,
            Self::HardwareCorrected => 17,
            Self::HardwareFatal => 18,
            Self::Wake => 1,
        }
    }

    pub fn channel(self) -> &'static str {
        match self {
            Self::Crash | Self::Hang => "Application",
            _ => "System",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Restart | Self::Bugcheck | Self::PlannedRestart => "restart",
            Self::Crash | Self::Hang => "app",
            Self::GraphicsReset => "graphics",
            Self::Update => "update",
            Self::ServiceRecovery | Self::ServiceStopped => "service",
            Self::HardwareCorrected | Self::HardwareFatal => "hardware",
            Self::Wake => "power",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub at: DateTime<Utc>,
    pub record: u64,
    pub kind: Kind,
    pub id: u16,
    pub detail: Detail,
}

#[derive(Clone, Debug, Default)]
pub struct Detail {
    pub application: Option<String>,
    pub module: Option<String>,
    pub exception: Option<u32>,
    pub stop_code: Option<u32>,
}

impl Detail {
    #[cfg(windows)]
    pub fn filename(value: &str) -> Option<String> {
        let name = value.rsplit(['/', '\\']).next()?.trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return None;
        }
        Some(name.chars().take(120).collect())
    }

    #[cfg(windows)]
    pub fn hex(value: &str) -> Option<u32> {
        let value = value.trim();
        let value = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);
        u32::from_str_radix(value, 16).ok()
    }

    pub fn stop_name(code: u32) -> Option<&'static str> {
        match code {
            0x0A => Some("IRQL_NOT_LESS_OR_EQUAL"),
            0x1A => Some("MEMORY_MANAGEMENT"),
            0x50 => Some("PAGE_FAULT_IN_NONPAGED_AREA"),
            0x9F => Some("DRIVER_POWER_STATE_FAILURE"),
            0xD1 => Some("DRIVER_IRQL_NOT_LESS_OR_EQUAL"),
            0x116 => Some("VIDEO_TDR_FAILURE"),
            0x124 => Some("WHEA_UNCORRECTABLE_ERROR"),
            0x133 => Some("DPC_WATCHDOG_VIOLATION"),
            0x139 => Some("KERNEL_SECURITY_CHECK_FAILURE"),
            _ => None,
        }
    }
}

impl Event {
    pub fn nearby_stop_error(&self, events: &[Self]) -> Option<u64> {
        if self.kind != Kind::Restart {
            return None;
        }
        let mut candidates = events.iter().filter(|event| {
            event.kind == Kind::Bugcheck
                && event.at >= self.at
                && event.at - self.at <= TimeDelta::minutes(3)
        });
        let candidate = candidates.next()?;
        if candidates.next().is_some()
            || events.iter().any(|event| {
                event.kind == Kind::Restart
                    && event.record != self.record
                    && event.at <= candidate.at
                    && candidate.at - event.at <= TimeDelta::minutes(3)
            })
        {
            return None;
        }
        Some(candidate.record)
    }
}

pub fn demo() -> Vec<Event> {
    demo_events(&[
        (0, Kind::Update),
        (3, Kind::Crash),
        (4, Kind::GraphicsReset),
        (6, Kind::Restart),
        (7, Kind::Bugcheck),
        (19, Kind::Restart),
        (24, Kind::Hang),
        (25, Kind::ServiceRecovery),
        (27, Kind::HardwareCorrected),
        (35, Kind::Crash),
    ])
}

pub struct DemoCase {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub next: &'static str,
    pub(crate) evidence: Kind,
    records: &'static [(i64, Kind)],
}

pub const DEMO_CASES: &[DemoCase] = &[
    DemoCase {
        id: "graphics",
        title: "Graphics-driver reset",
        summary: "The display flickers after the PC wakes. Windows records a graphics-driver recovery. The earlier wake provides timing context, and the recovery record leaves the underlying cause open.",
        next: "Open the Windows record for the display recovery and note when it happened. If you have an older inventory report, compare the graphics driver versions.",
        evidence: Kind::GraphicsReset,
        records: &[(0, Kind::Wake), (4, Kind::GraphicsReset)],
    },
    DemoCase {
        id: "application",
        title: "Crashing application",
        summary: "The fictional editor stops responding and later crashes. The crash names demo-renderer.dll and records an access violation. The faulting module tells you where Windows observed the failure.",
        next: "Open the Windows crash record. If the app crashes again, check whether Windows lists the same app, component and error code.",
        evidence: Kind::Crash,
        records: &[(3, Kind::Hang), (6, Kind::Crash)],
    },
    DemoCase {
        id: "restart",
        title: "Unexplained restart",
        summary: "Windows records an update and later starts after an unclean shutdown. The update is a timing clue. The restart record leaves the cause undetermined.",
        next: "Open the unexpected restart record. Look for a Windows stop error, hardware error or planned restart around the same time. If Windows recorded none of those, the cause is still unknown.",
        evidence: Kind::Restart,
        records: &[(0, Kind::Update), (6, Kind::Restart)],
    },
];

impl DemoCase {
    pub fn get(id: &str) -> Result<&'static Self, String> {
        DEMO_CASES
            .iter()
            .find(|case| case.id == id)
            .ok_or_else(|| "Choose the graphics, application or restart example.".into())
    }

    pub fn events(&self) -> Vec<Event> {
        demo_events(self.records)
    }
}

fn demo_events(records: &[(i64, Kind)]) -> Vec<Event> {
    let start = DateTime::parse_from_rfc3339("2026-11-22T06:08:00Z")
        .expect("valid demo date")
        .with_timezone(&Utc);
    records
        .iter()
        .copied()
        .enumerate()
        .map(|(index, (minute, kind))| Event {
            at: start + TimeDelta::minutes(minute),
            record: index as u64 + 1,
            kind,
            id: kind.demo_id(),
            detail: match kind {
                Kind::Crash => Detail {
                    application: Some("demo-editor.exe".into()),
                    module: Some("demo-renderer.dll".into()),
                    exception: Some(0xC0000005),
                    stop_code: None,
                },
                Kind::Bugcheck => Detail {
                    stop_code: Some(0x9F),
                    ..Default::default()
                },
                Kind::Hang => Detail {
                    application: Some("demo-editor.exe".into()),
                    ..Default::default()
                },
                _ => Detail::default(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fictional_cases_have_distinct_evidence_and_order() {
        for (id, expected) in [
            ("graphics", [Kind::Wake, Kind::GraphicsReset]),
            ("application", [Kind::Hang, Kind::Crash]),
            ("restart", [Kind::Update, Kind::Restart]),
        ] {
            let case = DemoCase::get(id).unwrap();
            let events = case.events();
            assert_eq!(
                events.iter().map(|event| event.kind).collect::<Vec<_>>(),
                expected
            );
            assert!(events[0].at < events[1].at);
            assert!(
                events
                    .iter()
                    .all(|event| Kind::from_event(event.kind.provider(), event.id)
                        == Some(event.kind))
            );
            assert!(!case.next.is_empty());
        }
        assert!(DemoCase::get("missing").is_err());
    }

    #[test]
    fn uses_provider_and_event_id() {
        assert_eq!(
            Kind::from_event("Application Error", 1000),
            Some(Kind::Crash)
        );
        assert_eq!(Kind::from_event("Unrelated provider", 1000), None);
        assert_eq!(
            Kind::from_event("Microsoft-Windows-Power-Troubleshooter", 1),
            Some(Kind::Wake)
        );
    }

    #[test]
    fn shows_nearby_stop_error_only_for_clear_pairing() {
        let mut events = demo();
        assert_eq!(events[3].nearby_stop_error(&events), Some(5));
        assert_eq!(events[5].nearby_stop_error(&events), None);
        let mut competing_restart = events[3].clone();
        competing_restart.record = 90;
        events.push(competing_restart);
        assert_eq!(events[3].nearby_stop_error(&events), None);
    }
}
