//! Phase machine — coarse sketch; night steps refined later.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Lobby,
    /// Characters assigned; first night not finished.
    FirstNight { step: NightStep },
    Day {
        day: u32,
        /// Discussion vs nominations open, etc.
        stage: DayStage,
    },
    Night {
        night: u32,
        step: NightStep,
    },
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayStage {
    Discussion,
    Nominations,
}

/// Index into the script night order, or named checkpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NightStep {
    /// Host/engine setup reminders (Drunk, red herring).
    SetupMarkers,
    /// Minion briefing, demon briefing, then character wakes…
    OrderIndex(usize),
    DawnPending,
}
