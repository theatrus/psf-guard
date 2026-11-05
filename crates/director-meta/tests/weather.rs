use psf_guard_director_meta::{MetaStore, Uuid, Error, weather::*};

const NOW: u64 = 1_800_000_000_000;
fn sample(profile: Uuid, cloud: Option<f64>) -> WeatherSample {
    WeatherSample { sample_id: Uuid::new_v4(), profile_id: profile, reported_at_ms: NOW,
        source_name: "NINA reported weather".into(), source_key: "a".repeat(64), connected: true,
        values: WeatherValues { cloud_percent: cloud, temperature_c: Some(7.5), ..Default::default() } }
}
fn fixture() -> (tempfile::TempDir, MetaStore, Uuid, Uuid) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = Uuid::new_v4(); let profile = Uuid::new_v4();
    store.create_rig(rig, "Weather rig").unwrap();
    (dir, store, rig, profile)
}

#[test]
fn reports_keep_units_and_missing_clouds_remain_unknown() {
    let (_dir, mut store, rig, profile) = fixture();
    store.record_weather(rig, sample(profile, None), NOW).unwrap();
    let view = store.rig_weather(rig, Some(profile), NOW).unwrap();
    assert_eq!(view.reported_cloud.state, CloudState::Unknown);
    assert_eq!(view.reported.unwrap().sample.values.temperature_c, Some(7.5));
    assert_eq!(view.forecast_cloud.state, CloudState::Unknown);
    assert_eq!(view.forecast_cloud.valid_for_ms, 0);
}

#[test]
fn retries_and_old_samples_do_not_refresh_weather_or_invalidate_plans() {
    let (_dir, mut store, rig, profile) = fixture();
    let before = store.planning_rows_written();
    let report = sample(profile, Some(15.));
    assert!(store.record_weather(rig, report.clone(), NOW).unwrap());
    assert!(!store.record_weather(rig, report.clone(), NOW + 30_000).unwrap());
    assert_eq!(store.weather_observation(rig).unwrap().unwrap().received_at_ms, NOW);
    let mut old = sample(profile, Some(0.)); old.reported_at_ms -= 1;
    assert!(!store.record_weather(rig, old, NOW + 31_000).unwrap());
    assert_eq!(store.planning_rows_written(), before);
    let mut conflicting = report; conflicting.values.cloud_percent = Some(80.);
    assert!(matches!(store.record_weather(rig, conflicting, NOW + 40_000), Err(Error::Conflict)));
}

#[test]
fn stale_disconnected_wrong_profile_and_unsupported_clouds_never_mean_clear() {
    let (_dir, mut store, rig, profile) = fixture();
    store.record_weather(rig, sample(profile, Some(0.)), NOW).unwrap();
    assert_eq!(store.rig_weather(rig, Some(profile), NOW).unwrap().reported_cloud.state, CloudState::Clear);
    assert_eq!(store.rig_weather(rig, Some(profile), NOW + WEATHER_MAX_AGE_MS).unwrap().reported_cloud.state, CloudState::Unknown);
    assert_eq!(store.rig_weather(rig, Some(Uuid::new_v4()), NOW).unwrap().reported_cloud.state, CloudState::Unknown);
    assert_eq!(store.rig_weather(rig, Some(profile), NOW - 1).unwrap().reported_cloud.state, CloudState::Unknown);
    let mut off = sample(profile, Some(0.)); off.connected = false; off.reported_at_ms += 1;
    store.record_weather(rig, off, NOW + 1).unwrap();
    assert_eq!(store.rig_weather(rig, Some(profile), NOW + 1).unwrap().reported_cloud.state, CloudState::Unknown);
}

#[test]
fn history_coalesces_by_minute_and_survives_reopening() {
    let (dir, mut store, rig, profile) = fixture();
    store.record_weather(rig, sample(profile, Some(40.)), NOW).unwrap();
    let mut next = sample(profile, Some(20.)); next.reported_at_ms += 10_000;
    store.record_weather(rig, next, NOW + 10_000).unwrap();
    assert_eq!(store.weather_history(rig, NOW, NOW + 10_000).unwrap().len(), 1);
    drop(store);
    let store = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    assert_eq!(store.rig_weather(rig, Some(profile), NOW + 10_000).unwrap().reported_cloud.state, CloudState::Clear);
}

#[test]
fn invalid_numeric_values_and_future_samples_are_refused() {
    let (_dir, mut store, rig, profile) = fixture();
    for cloud in [-1., 101., f64::NAN, f64::INFINITY] {
        assert!(store.record_weather(rig, sample(profile, Some(cloud)), NOW).is_err());
    }
    let mut future = sample(profile, Some(0.)); future.reported_at_ms += 5_001;
    assert!(store.record_weather(rig, future, NOW).is_err());
    assert!(store.weather_observation(rig).unwrap().is_none());
}

#[test]
fn centralized_site_forecast_and_reported_clouds_use_one_threshold_without_blending() {
    use psf_guard_director_core::priority::Scope;
    use psf_guard_director_meta::{preferences::Settings, site_profile::SiteProfile, site_forecast::*};
    let (_dir, mut store, rig, profile) = fixture();
    let site = store.create_site(Uuid::new_v4(), "Shared paid forecast site").unwrap().id;
    let mut settings = Settings::empty(Scope::Rig, rig); settings.site_id = Some(site);
    store.save_observing_settings(&settings).unwrap();
    let mut setup = SiteProfile::empty(site, NOW);
    setup.forecast = Some(ForecastSettings { provider: ForecastProvider::OpenMeteo, cloud_threshold_percent: 25 });
    store.save_site_profile(&setup, 0).unwrap();
    let forecast = SiteForecast { site_id: site, provider: ForecastProvider::OpenMeteo, fetched_at_ms: NOW,
        expires_at_ms: NOW + 3600000, checked_at_ms: None, refresh_after_ms: None, last_modified: None,
        hours: vec![ForecastHour { start_ms: NOW, end_ms: NOW + 3600000, cloud_total: 10., cloud_low: None, cloud_mid: None, cloud_high: None }] };
    store.save_site_forecast(&forecast).unwrap();
    store.record_weather(rig, sample(profile, Some(70.)), NOW).unwrap();
    let view = store.rig_weather(rig, Some(profile), NOW).unwrap();
    assert_eq!(view.cloud_threshold_percent, 25);
    assert_eq!(view.reported_cloud.state, CloudState::Cloudy);
    assert_eq!(view.forecast_cloud.state, CloudState::Clear);
    assert_eq!(view.forecast.unwrap().provider, ForecastProvider::OpenMeteo);
    assert_eq!(store.rig_weather(rig, Some(profile), NOW + 3600000).unwrap().forecast_cloud.state, CloudState::Unknown);
}

#[test]
fn another_paired_profile_cannot_replace_the_current_profiles_weather() {
    let (_dir, mut store, rig, profile) = fixture();
    store.record_weather(rig, sample(profile, Some(80.)), NOW).unwrap();
    let other = Uuid::new_v4();
    store.record_weather(rig, sample(other, Some(0.)), NOW + 1).unwrap();
    assert_eq!(store.rig_weather(rig, Some(profile), NOW + 1).unwrap().reported_cloud.state, CloudState::Cloudy);
    assert_eq!(store.rig_weather(rig, Some(other), NOW + 1).unwrap().reported_cloud.state, CloudState::Clear);
    assert_eq!(store.weather_history(rig, NOW, NOW + 1).unwrap().len(), 2);
}
