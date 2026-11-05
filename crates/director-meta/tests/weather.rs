use psf_guard_director_meta::{weather::*, Error, MetaStore, Uuid};

const NOW: u64 = 1_800_000_000_000;
fn sample(profile: Uuid, cloud: Option<f64>) -> WeatherSample {
    WeatherSample {
        sample_id: Uuid::new_v4(),
        profile_id: profile,
        reported_at_ms: NOW,
        source_name: "NINA reported weather".into(),
        source_key: "a".repeat(64),
        connected: true,
        values: WeatherValues {
            cloud_percent: cloud,
            temperature_celsius: Some(7.5),
            ..Default::default()
        },
    }
}
fn fixture() -> (tempfile::TempDir, MetaStore, Uuid, Uuid) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let rig = Uuid::new_v4();
    let profile = Uuid::new_v4();
    store.create_rig(rig, "Weather rig").unwrap();
    (dir, store, rig, profile)
}

#[test]
fn reports_keep_units_and_missing_clouds_remain_unknown() {
    let (_dir, mut store, rig, profile) = fixture();
    store
        .record_weather(rig, sample(profile, None), NOW)
        .unwrap();
    let view = store.rig_weather(rig, Some(profile), NOW).unwrap();
    assert_eq!(view.reported_cloud.state, CloudState::Unknown);
    assert_eq!(
        view.reported.unwrap().sample.values.temperature_celsius,
        Some(7.5)
    );
    assert_eq!(view.forecast_conditions.state, CloudState::Unknown);
    assert_eq!(view.forecast_conditions.valid_for_ms, 0);
}

#[test]
fn retries_and_old_samples_do_not_refresh_weather_or_invalidate_plans() {
    let (_dir, mut store, rig, profile) = fixture();
    let before = store.planning_rows_written();
    let report = sample(profile, Some(15.));
    assert!(store.record_weather(rig, report.clone(), NOW).unwrap());
    assert!(!store
        .record_weather(rig, report.clone(), NOW + 30_000)
        .unwrap());
    assert_eq!(
        store
            .weather_observation(rig)
            .unwrap()
            .unwrap()
            .received_at_ms,
        NOW
    );
    let mut old = sample(profile, Some(0.));
    old.reported_at_ms -= 1;
    assert!(!store.record_weather(rig, old, NOW + 31_000).unwrap());
    assert_eq!(store.planning_rows_written(), before);
    let mut conflicting = report;
    conflicting.values.cloud_percent = Some(80.);
    assert!(matches!(
        store.record_weather(rig, conflicting, NOW + 40_000),
        Err(Error::Conflict)
    ));
}

#[test]
fn stale_disconnected_wrong_profile_and_unsupported_clouds_never_mean_clear() {
    let (_dir, mut store, rig, profile) = fixture();
    store
        .record_weather(rig, sample(profile, Some(0.)), NOW)
        .unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Clear
    );
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW + WEATHER_MAX_AGE_MS)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Unknown
    );
    assert_eq!(
        store
            .rig_weather(rig, Some(Uuid::new_v4()), NOW)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Unknown
    );
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW - 1)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Unknown
    );
    let mut off = sample(profile, Some(0.));
    off.connected = false;
    off.reported_at_ms += 1;
    store.record_weather(rig, off, NOW + 1).unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW + 1)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Unknown
    );
}

#[test]
fn history_coalesces_by_minute_and_survives_reopening() {
    let (dir, mut store, rig, profile) = fixture();
    store
        .record_weather(rig, sample(profile, Some(40.)), NOW)
        .unwrap();
    let mut next = sample(profile, Some(20.));
    next.reported_at_ms += 10_000;
    store.record_weather(rig, next, NOW + 10_000).unwrap();
    assert_eq!(
        store.weather_history(rig, NOW, NOW + 10_000).unwrap().len(),
        1
    );
    drop(store);
    let store = MetaStore::open(&dir.path().join("meta.sqlite")).unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW + 10_000)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Clear
    );
}

#[test]
fn invalid_numeric_values_and_future_samples_are_refused() {
    let (_dir, mut store, rig, profile) = fixture();
    for cloud in [-1., 101., f64::NAN, f64::INFINITY] {
        assert!(store
            .record_weather(rig, sample(profile, Some(cloud)), NOW)
            .is_err());
    }
    let mut future = sample(profile, Some(0.));
    future.reported_at_ms += 5_001;
    assert!(store.record_weather(rig, future, NOW).is_err());
    assert!(store.weather_observation(rig).unwrap().is_none());
}

#[test]
fn centralized_site_forecast_and_reported_clouds_use_one_threshold_without_blending() {
    use psf_guard_director_core::priority::Scope;
    use psf_guard_director_meta::{
        preferences::Settings, site_forecast::*, site_profile::SiteProfile,
    };
    let (_dir, mut store, rig, profile) = fixture();
    let site = store
        .create_site(Uuid::new_v4(), "Shared paid forecast site")
        .unwrap()
        .id;
    let mut settings = Settings::empty(Scope::Rig, rig);
    settings.site_id = Some(site);
    store.save_observing_settings(&settings).unwrap();
    let mut setup = SiteProfile::empty(site, NOW);
    setup.location = Some(psf_guard_director_meta::profile::Reported {
        value: psf_guard_director_core::visibility::Site {
            latitude_degrees: 38.66,
            longitude_degrees: -121.16,
            elevation_meters: 10.0,
        },
        source: psf_guard_director_meta::profile::Source::Manual {},
        reported_at_ms: NOW,
    });
    setup.forecast = Some(ForecastSettings {
        provider: ForecastProvider::OpenMeteo,
        cloud_threshold_percent: 25,
        dew_limit_celsius: None,
    });
    store.save_site_profile(&setup, 0).unwrap();
    let forecast = SiteForecast {
        site_id: site,
        provider: ForecastProvider::OpenMeteo,
        fetched_at_ms: NOW,
        expires_at_ms: NOW + 3600000,
        checked_at_ms: None,
        refresh_after_ms: None,
        last_modified: None,
        hours: vec![ForecastHour {
            start_ms: NOW,
            end_ms: NOW + 3600000,
            cloud_total: 10.,
            cloud_low: None,
            cloud_mid: None,
            cloud_high: None,
            temperature_celsius: Some(7.),
            dew_point_celsius: Some(6.),
            humidity_percent: Some(93.),
        }],
    };
    store.save_site_forecast(&forecast).unwrap();
    store
        .record_weather(rig, sample(profile, Some(70.)), NOW)
        .unwrap();
    let view = store.rig_weather(rig, Some(profile), NOW).unwrap();
    assert_eq!(view.cloud_threshold_percent, 25);
    assert_eq!(view.reported_cloud.state, CloudState::Cloudy);
    assert_eq!(view.forecast_conditions.state, CloudState::Clear);
    assert_eq!(view.forecast.unwrap().provider, ForecastProvider::OpenMeteo);
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW - 1)
            .unwrap()
            .forecast_conditions
            .state,
        CloudState::Unknown
    );
    let mut setup = store.site_profile(site).unwrap().unwrap();
    setup.forecast.as_mut().unwrap().dew_limit_celsius = Some(2);
    store.save_site_profile(&setup, setup.revision).unwrap();
    let dewy = store.rig_weather(rig, Some(profile), NOW).unwrap();
    assert_eq!(dewy.forecast_conditions.state, CloudState::Dewy);
    assert_eq!(dewy.forecast_conditions.reason, "dew_limit_reached");
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW + 3600000)
            .unwrap()
            .forecast_conditions
            .state,
        CloudState::Unknown
    );
    let mut gap = forecast;
    gap.fetched_at_ms += 1;
    gap.hours[0].end_ms = NOW + 1_000;
    let mut next = gap.hours[0];
    next.start_ms = NOW + 2_000;
    next.end_ms = NOW + 3_600_000;
    gap.hours.push(next);
    store.save_site_forecast(&gap).unwrap();
    let uncovered = store.rig_weather(rig, Some(profile), NOW + 1_500).unwrap();
    assert_eq!(
        uncovered.forecast_conditions.reason,
        "forecast_does_not_cover_now"
    );
    assert_eq!(uncovered.forecast_conditions.valid_for_ms, 0);
    let mut setup = store.site_profile(site).unwrap().unwrap();
    setup.forecast.as_mut().unwrap().provider = ForecastProvider::MetNorway;
    store.save_site_profile(&setup, setup.revision).unwrap();
    assert!(store
        .rig_weather(rig, Some(profile), NOW + 3_000)
        .unwrap()
        .forecast
        .is_none());
}

#[test]
fn another_paired_profile_cannot_replace_the_current_profiles_weather() {
    let (_dir, mut store, rig, profile) = fixture();
    store
        .record_weather(rig, sample(profile, Some(80.)), NOW)
        .unwrap();
    let other = Uuid::new_v4();
    store
        .record_weather(rig, sample(other, Some(0.)), NOW + 1)
        .unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW + 1)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Cloudy
    );
    assert_eq!(
        store
            .rig_weather(rig, Some(other), NOW + 1)
            .unwrap()
            .reported_cloud
            .state,
        CloudState::Clear
    );
    assert_eq!(store.weather_history(rig, NOW, NOW + 1).unwrap().len(), 2);
}

fn forecast_fixture(
    provider: psf_guard_director_meta::site_forecast::ForecastProvider,
) -> (tempfile::TempDir, MetaStore, Uuid, Uuid, Uuid) {
    use psf_guard_director_core::{priority::Scope, visibility::Site};
    use psf_guard_director_meta::{
        preferences::Settings,
        profile::{Reported, Source},
        site_forecast::*,
        site_profile::SiteProfile,
    };
    let (dir, mut store, rig, profile) = fixture();
    let site = Uuid::new_v4();
    store.create_site(site, "California forecast site").unwrap();
    let mut settings = Settings::empty(Scope::Rig, rig);
    settings.site_id = Some(site);
    store.save_observing_settings(&settings).unwrap();
    let mut setup = SiteProfile::empty(site, NOW);
    setup.location = Some(Reported {
        value: Site {
            latitude_degrees: 38.66,
            longitude_degrees: -121.16,
            elevation_meters: 10.0,
        },
        source: Source::Manual {},
        reported_at_ms: NOW,
    });
    setup.forecast = Some(ForecastSettings {
        provider,
        cloud_threshold_percent: 25,
        dew_limit_celsius: None,
    });
    store.save_site_profile(&setup, 0).unwrap();
    store
        .save_site_forecast(&SiteForecast {
            site_id: site,
            provider,
            fetched_at_ms: NOW,
            expires_at_ms: NOW + 3600000,
            checked_at_ms: None,
            refresh_after_ms: None,
            last_modified: None,
            hours: vec![ForecastHour {
                start_ms: NOW,
                end_ms: NOW + 3600000,
                cloud_total: 0.0,
                cloud_low: None,
                cloud_mid: None,
                cloud_high: None,
                temperature_celsius: None,
                dew_point_celsius: None,
                humidity_percent: None,
            }],
        })
        .unwrap();
    store
        .record_weather(rig, sample(profile, Some(10.0)), NOW)
        .unwrap();
    (dir, store, rig, profile, site)
}

#[test]
fn mismatched_forecasts_remain_unknown_for_both_providers() {
    use psf_guard_director_core::visibility::Site;
    use psf_guard_director_meta::{
        profile::{Reported, RigProfile, Source},
        site_forecast::ForecastProvider,
    };
    for provider in [ForecastProvider::OpenMeteo, ForecastProvider::MetNorway] {
        let (_dir, mut store, rig, profile, site) = forecast_fixture(provider);
        let mut own = RigProfile::empty(rig, NOW);
        own.site = Some(Reported {
            value: Site {
                latitude_degrees: -33.45,
                longitude_degrees: -70.66,
                elevation_meters: 1000.0,
            },
            source: Source::Plugin {},
            reported_at_ms: NOW,
        });
        store.save_rig_profile(&own, 0).unwrap();
        for cloud in [0.0, 100.0] {
            let mut forecast = store.site_forecast(site).unwrap().unwrap();
            forecast.fetched_at_ms += 1;
            forecast.hours[0].cloud_total = cloud;
            store.save_site_forecast(&forecast).unwrap();
            let view = store.rig_weather(rig, Some(profile), NOW + 100).unwrap();
            assert_eq!(view.forecast_conditions.state, CloudState::Unknown);
            assert_eq!(
                view.forecast_conditions.reason,
                "forecast_location_mismatch"
            );
            assert_eq!(view.forecast_conditions.valid_for_ms, 0);
            assert!(view.forecast.unwrap().fresh);
            assert_eq!(view.reported_cloud.state, CloudState::Clear);
            assert_eq!(view.cloud_threshold_percent, 25);
        }
    }
}

#[test]
fn forecast_location_uses_inheritance_and_the_providers_request_precision() {
    use psf_guard_director_core::visibility::Site;
    use psf_guard_director_meta::{
        profile::{Reported, RigProfile, Source},
        site_forecast::ForecastProvider,
    };
    let (_dir, mut store, rig, profile, _site) = forecast_fixture(ForecastProvider::OpenMeteo);
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW)
            .unwrap()
            .forecast_conditions
            .state,
        CloudState::Clear
    );
    let mut own = RigProfile::empty(rig, NOW);
    own.site = Some(Reported {
        value: Site {
            latitude_degrees: 38.6601,
            longitude_degrees: -121.1601,
            elevation_meters: 1000.0,
        },
        source: Source::Manual {},
        reported_at_ms: NOW,
    });
    let mut own = store.save_rig_profile(&own, 0).unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW)
            .unwrap()
            .forecast_conditions
            .state,
        CloudState::Clear
    );
    own.site.as_mut().unwrap().value.latitude_degrees = 38.666;
    let mut own = store.save_rig_profile(&own, own.revision).unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW)
            .unwrap()
            .forecast_conditions
            .reason,
        "forecast_location_mismatch"
    );
    own.site = None;
    store.save_rig_profile(&own, own.revision).unwrap();
    assert_eq!(
        store
            .rig_weather(rig, Some(profile), NOW)
            .unwrap()
            .forecast_conditions
            .state,
        CloudState::Clear
    );
}

#[test]
fn a_forecast_without_a_known_location_cannot_clear_the_rig() {
    use psf_guard_director_meta::site_forecast::ForecastProvider;
    let (_dir, mut store, rig, profile, site) = forecast_fixture(ForecastProvider::OpenMeteo);
    let mut setup = store.site_profile(site).unwrap().unwrap();
    setup.location = None;
    store.save_site_profile(&setup, setup.revision).unwrap();
    let view = store.rig_weather(rig, Some(profile), NOW).unwrap();
    assert_eq!(view.forecast_conditions.state, CloudState::Unknown);
    assert_eq!(
        view.forecast_conditions.reason,
        "forecast_location_unavailable"
    );
    assert_eq!(view.forecast_conditions.valid_for_ms, 0);
    assert_eq!(view.reported_cloud.state, CloudState::Clear);
}
