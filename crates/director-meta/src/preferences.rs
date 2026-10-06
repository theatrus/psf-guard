//! Mutable observing intent. Effective values are frozen into issued programs.
use super::*;
use psf_guard_director_core::priority::{
    self, Layer, Overrides, Policy, Preset, ResolvedPolicy, Scope, Source,
};

/// Target Scheduler's per-project scheduling limits, as Director plans them.
/// Each is a default set once at a scope (global, site, rig) or an override
/// for one project; `None` inherits. Activation writes the resolved values
/// into each rig's Target Scheduler project, so they also hold when Target
/// Scheduler runs a rig without Director.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SchedulingOverrides {
    /// Shortest time worth starting the project for, in minutes.
    pub minimum_time_minutes: Option<u32>,
    /// Lowest altitude to image at, in degrees.
    pub minimum_altitude_degrees: Option<f64>,
    /// Highest altitude to image at, in degrees; 0 means no upper limit.
    pub maximum_altitude_degrees: Option<f64>,
    /// Use the profile's custom horizon instead of a flat minimum altitude.
    pub use_custom_horizon: Option<bool>,
    /// Degrees added to the custom horizon.
    pub horizon_offset_degrees: Option<f64>,
    /// Minutes either side of the meridian to avoid; 0 turns it off.
    pub meridian_window_minutes: Option<u32>,
    /// Exposures before switching filter; 0 lets Target Scheduler decide.
    pub filter_switch_frequency: Option<u32>,
    /// Exposures between dithers; 0 dithers per the exposure template.
    pub dither_every: Option<u32>,
    /// Let Target Scheduler order exposures for the best conditions.
    pub smart_exposure_order: Option<bool>,
}

/// Every scheduling limit with a value: an override, else a default from a
/// scope, else Target Scheduler's own default.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SchedulingValues {
    pub minimum_time_minutes: u32,
    pub minimum_altitude_degrees: f64,
    pub maximum_altitude_degrees: f64,
    pub use_custom_horizon: bool,
    pub horizon_offset_degrees: f64,
    pub meridian_window_minutes: u32,
    pub filter_switch_frequency: u32,
    pub dither_every: u32,
    pub smart_exposure_order: bool,
}

impl Default for SchedulingValues {
    /// What Target Scheduler gives a new project.
    fn default() -> Self {
        Self {
            minimum_time_minutes: 30,
            minimum_altitude_degrees: 0.0,
            maximum_altitude_degrees: 0.0,
            use_custom_horizon: false,
            horizon_offset_degrees: 0.0,
            meridian_window_minutes: 0,
            filter_switch_frequency: 0,
            dither_every: 0,
            smart_exposure_order: false,
        }
    }
}

/// The resolved limits and, for each, the scope that set it: `None` is
/// Target Scheduler's default, which activation writes only into a project
/// it creates, never over a value set by hand.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResolvedScheduling {
    pub values: SchedulingValues,
    pub sources: std::collections::BTreeMap<String, Source>,
}

impl SchedulingOverrides {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn validate(&self) -> Result<(), Error> {
        let altitude =
            |value: Option<f64>| value.is_none_or(|v| v.is_finite() && (0.0..=90.0).contains(&v));
        let offset = self
            .horizon_offset_degrees
            .is_none_or(|v| v.is_finite() && (-90.0..=90.0).contains(&v));
        let within = |value: Option<u32>, max: u32| value.is_none_or(|v| v <= max);
        let ordered = match (self.minimum_altitude_degrees, self.maximum_altitude_degrees) {
            (Some(low), Some(high)) => high == 0.0 || high > low,
            _ => true,
        };
        if altitude(self.minimum_altitude_degrees)
            && altitude(self.maximum_altitude_degrees)
            && offset
            && ordered
            && within(self.minimum_time_minutes, 24 * 60)
            && within(self.meridian_window_minutes, 12 * 60)
            && within(self.filter_switch_frequency, 10_000)
            && within(self.dither_every, 10_000)
        {
            Ok(())
        } else {
            Err(Error::InvalidInput)
        }
    }
}

/// Resolve the limits through `layers`, most general first: each set value
/// replaces what came before it.
pub fn resolve_scheduling(layers: &[(&SchedulingOverrides, Source)]) -> ResolvedScheduling {
    let mut values = SchedulingValues::default();
    let mut sources = std::collections::BTreeMap::new();
    for (overrides, source) in layers {
        macro_rules! take {
            ($field:ident) => {
                if let Some(value) = overrides.$field {
                    values.$field = value;
                    sources.insert(stringify!($field).to_string(), source.clone());
                }
            };
        }
        take!(minimum_time_minutes);
        take!(minimum_altitude_degrees);
        take!(maximum_altitude_degrees);
        take!(use_custom_horizon);
        take!(horizon_offset_degrees);
        take!(meridian_window_minutes);
        take!(filter_switch_frequency);
        take!(dither_every);
        take!(smart_exposure_order);
    }
    ResolvedScheduling { values, sources }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub scope: Scope,
    pub scope_id: Uuid,
    pub revision: u64,
    pub overrides: Overrides,
    /// Global/site/rig mode. Projects override preferences, not executor capability.
    pub enabled: Option<bool>,
    /// Explicit site association for a rig; never guessed from coordinates/names.
    pub site_id: Option<Uuid>,
    /// Ordered project identities; None inherits, an empty list ranks none first.
    #[serde(default)]
    pub project_order: Option<Vec<Uuid>>,
    /// Target Scheduler scheduling limits set at this scope.
    #[serde(default, skip_serializing_if = "SchedulingOverrides::is_empty")]
    pub scheduling: SchedulingOverrides,
}

impl Settings {
    pub fn empty(scope: Scope, scope_id: Uuid) -> Self {
        Self {
            scope,
            scope_id,
            revision: 0,
            overrides: Overrides::default(),
            enabled: None,
            site_id: None,
            project_order: None,
            scheduling: SchedulingOverrides::default(),
        }
    }
    fn source(&self) -> Source {
        Source {
            scope: self.scope,
            id: self.scope_id.to_string(),
            revision: self.revision + 1,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Effective {
    pub enabled: bool,
    pub resolved: ResolvedPolicy,
    pub settings: Vec<Settings>,
    pub project_order: Option<Vec<Uuid>>,
    pub order_source: Option<Source>,
    /// Target Scheduler limits for this rig, and this project when given.
    pub scheduling: ResolvedScheduling,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch("CREATE TABLE observing_preferences(scope TEXT NOT NULL, scope_id TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(scope,scope_id));")?;
    Ok(())
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::Site => "site",
        Scope::Rig => "rig",
        Scope::Project => "project",
    }
}

/// The largest stored settings record: room for a project order that ranks
/// every plan, with its policy and limits beside it.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

fn check_scope(conn: &Connection, instance: Uuid, scope: Scope, id: Uuid) -> Result<(), Error> {
    valid_id(id)?;
    let exists = match scope {
        Scope::Global => id == instance,
        Scope::Site => read_named(conn, Kind::Site, id)?.is_some(),
        Scope::Rig => read_named(conn, Kind::Rig, id)?.is_some(),
        Scope::Project => read_named(conn, Kind::Project, id)?.is_some(),
    };
    if !exists {
        return Err(Error::NotFound);
    }
    Ok(())
}

fn validate(conn: &Connection, instance: Uuid, value: &Settings) -> Result<(), Error> {
    check_scope(conn, instance, value.scope, value.scope_id)?;
    if value.revision >= i64::MAX as u64
        || value.scope == Scope::Project && value.enabled.is_some()
        || value.scope != Scope::Rig && value.site_id.is_some()
        || value.scope == Scope::Project && value.project_order.is_some()
    {
        return Err(Error::InvalidInput);
    }
    if let Some(site) = value.site_id {
        check_scope(conn, instance, Scope::Site, site)?;
    }
    if let Some(order) = &value.project_order {
        let unique: std::collections::BTreeSet<_> = order.iter().collect();
        if order.len() > crate::MAX_PLANS
            || unique.len() != order.len()
            || order.iter().any(Uuid::is_nil)
        {
            return Err(Error::InvalidInput);
        }
    }
    value
        .overrides
        .apply_to(Policy::preset(Preset::Balanced))
        .map_err(|_| Error::InvalidInput)?;
    value.scheduling.validate()?;
    Ok(())
}

fn read(conn: &Connection, instance: Uuid, scope: Scope, id: Uuid) -> Result<Settings, Error> {
    check_scope(conn, instance, scope, id)?;
    let payload: Option<String> = conn
        .query_row(
            "SELECT substr(payload,1,?3) FROM observing_preferences WHERE scope=?1 AND scope_id=?2",
            params![
                scope_name(scope),
                id.to_string(),
                (MAX_SETTINGS_BYTES + 1) as i64
            ],
            |r| r.get(0),
        )
        .optional()?;
    let Some(payload) = payload else {
        return Ok(Settings::empty(scope, id));
    };
    if payload.len() > MAX_SETTINGS_BYTES {
        return Err(Error::CorruptDatabase);
    }
    let value: Settings = serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
    if value.scope != scope || value.scope_id != id || value.revision == 0 {
        return Err(Error::CorruptDatabase);
    }
    validate(conn, instance, &value).map_err(|_| Error::CorruptDatabase)?;
    Ok(value)
}

// Parent edits must not leave a child with no enabled scoring factors. Check
// the saved hierarchy before committing so the UI can retain a valid draft.
fn validate_hierarchy(conn: &Connection, instance: Uuid) -> Result<(), Error> {
    let mut statement = conn.prepare("SELECT scope,scope_id FROM observing_preferences")?;
    let keys = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut settings = vec![];
    for key in keys {
        let (scope, id) = key?;
        let scope = match scope.as_str() {
            "global" => Scope::Global,
            "site" => Scope::Site,
            "rig" => Scope::Rig,
            "project" => Scope::Project,
            _ => return Err(Error::CorruptDatabase),
        };
        settings.push(read(
            conn,
            instance,
            scope,
            Uuid::parse_str(&id).map_err(|_| Error::CorruptDatabase)?,
        )?);
    }
    let apply = |s: &Settings, p: Policy| s.overrides.apply_to(p).map_err(|_| Error::InvalidInput);
    let global = apply(
        &read(conn, instance, Scope::Global, instance)?,
        Policy::preset(Preset::Balanced),
    )?;
    let mut bases = vec![global.clone()];
    let mut sites = std::collections::BTreeMap::new();
    for s in settings.iter().filter(|s| s.scope == Scope::Site) {
        let policy = apply(s, global.clone())?;
        sites.insert(s.scope_id, policy.clone());
        bases.push(policy);
    }
    for s in settings.iter().filter(|s| s.scope == Scope::Rig) {
        let parent = s.site_id.and_then(|id| sites.get(&id)).unwrap_or(&global);
        bases.push(apply(s, parent.clone())?);
    }
    for s in settings.iter().filter(|s| s.scope == Scope::Project) {
        for base in &bases {
            apply(s, base.clone())?;
        }
    }
    Ok(())
}

pub(crate) fn attach_project(
    conn: &Connection,
    instance: Uuid,
    into: Uuid,
    from: Uuid,
) -> Result<(), Error> {
    let own = read(conn, instance, Scope::Project, into)?;
    let mut absorbed = read(conn, instance, Scope::Project, from)?;
    if own.revision == 0 && absorbed.revision > 0 {
        absorbed.scope_id = into;
        absorbed.revision = 1;
        conn.execute(
            "INSERT INTO observing_preferences VALUES('project',?1,?2)",
            params![
                into.to_string(),
                serde_json::to_string(&absorbed).map_err(|_| Error::InvalidInput)?
            ],
        )?;
    }
    conn.execute(
        "DELETE FROM observing_preferences WHERE scope='project' AND scope_id=?1",
        [from.to_string()],
    )?;
    let mut statement = conn.prepare("SELECT payload FROM observing_preferences")?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for payload in rows {
        let mut settings: Settings =
            serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
        if let Some(order) = &mut settings.project_order {
            if !order.contains(&from) {
                continue;
            }
            if order.contains(&into) {
                order.retain(|id| *id != from);
            } else {
                for id in order {
                    if *id == from {
                        *id = into;
                    }
                }
            }
            settings.revision = settings
                .revision
                .checked_add(1)
                .filter(|r| *r < i64::MAX as u64)
                .ok_or(Error::InvalidInput)?;
            conn.execute(
                "UPDATE observing_preferences SET payload=?1 WHERE scope=?2 AND scope_id=?3",
                params![
                    serde_json::to_string(&settings).map_err(|_| Error::InvalidInput)?,
                    scope_name(settings.scope),
                    settings.scope_id.to_string()
                ],
            )?;
        }
    }
    Ok(())
}

impl MetaStore {
    pub fn observing_settings(&self, scope: Scope, id: Uuid) -> Result<Settings, Error> {
        read(&self.connection, self.instance_id, scope, id)
    }

    pub fn save_observing_settings(&mut self, input: &Settings) -> Result<Settings, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate(&tx, self.instance_id, input)?;
        if let Some(order) = &input.project_order {
            for id in order {
                check_scope(&tx, self.instance_id, Scope::Project, *id)?;
            }
        }
        let previous = read(&tx, self.instance_id, input.scope, input.scope_id)?;
        if previous.revision != input.revision {
            return Err(Error::Conflict);
        }
        let mut saved = input.clone();
        saved.revision += 1;
        let payload = serde_json::to_string(&saved).map_err(|_| Error::InvalidInput)?;
        // Never store what the next read would refuse as corrupt.
        if payload.len() > MAX_SETTINGS_BYTES {
            return Err(Error::InvalidInput);
        }
        tx.execute("INSERT INTO observing_preferences VALUES(?1,?2,?3) ON CONFLICT(scope,scope_id) DO UPDATE SET payload=excluded.payload",
            params![scope_name(saved.scope),saved.scope_id.to_string(),payload])?;
        validate_hierarchy(&tx, self.instance_id)?;
        tx.commit()?;
        Ok(saved)
    }

    pub fn effective_observing_preferences(
        &self,
        rig: Uuid,
        project: Option<Uuid>,
    ) -> Result<Effective, Error> {
        let global = self.observing_settings(Scope::Global, self.instance_id)?;
        let rig_settings = self.observing_settings(Scope::Rig, rig)?;
        let mut settings = vec![global.clone()];
        if let Some(site) = rig_settings.site_id {
            settings.push(self.observing_settings(Scope::Site, site)?);
        }
        settings.push(rig_settings);
        if let Some(project) = project {
            settings.push(self.observing_settings(Scope::Project, project)?);
        }
        let policy = global
            .overrides
            .apply_to(Policy::preset(Preset::Balanced))
            .map_err(|_| Error::InvalidInput)?;
        let layers: Vec<_> = settings
            .iter()
            .skip(1)
            .map(|s| Layer {
                source: s.source(),
                overrides: s.overrides.clone(),
            })
            .collect();
        let resolved =
            priority::resolve(policy, global.source(), &layers).map_err(|_| Error::InvalidInput)?;
        let enabled = settings
            .iter()
            .filter_map(|s| s.enabled)
            .next_back()
            .unwrap_or(false);
        let ranked = settings.iter().rev().find(|s| s.project_order.is_some());
        let project_order = ranked.and_then(|s| s.project_order.clone());
        let order_source = ranked.map(Settings::source);
        let scheduling = resolve_scheduling(
            &settings
                .iter()
                .map(|s| (&s.scheduling, s.source()))
                .collect::<Vec<_>>(),
        );
        Ok(Effective {
            enabled,
            resolved,
            settings,
            project_order,
            order_source,
            scheduling,
        })
    }
}
