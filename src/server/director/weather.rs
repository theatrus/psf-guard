//! Weather travels through the existing rig pairing. Provider credentials and
//! refreshes remain on PSF Guard; these routes only read its cached forecast.
use super::*;
use axum::http::{header::AUTHORIZATION, HeaderMap};
use psf_guard_director_meta::weather::{RigWeather, WeatherObservation, WeatherSample};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/weather", get(show).post(report))
        .route("/rigs/{rig}/weather/history", get(history))
        .layer(DefaultBodyLimit::max(8192))
}

fn profile(headers: &HeaderMap) -> Result<Option<Uuid>, Error> {
    headers
        .get("x-psf-director-profile")
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or(Error::Invalid)
        })
        .transpose()
}

async fn show(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let profile = profile(&headers)?;
    let view = enabled(&state)?
        .query(move |store| store.rig_weather(rig, profile, now_ms()))
        .await?;
    Ok(pairing::no_store(Json(ApiResponse::success(view))))
}

#[derive(Serialize)]
struct WeatherAnswer {
    sample_id: Uuid,
    profile_id: Uuid,
    stored: bool,
    weather: RigWeather,
}

async fn report(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
    Json(sample): Json<WeatherSample>,
) -> Result<Response, Error> {
    let service = enabled(&state)?;
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("bearer") && token.starts_with("psfdrc_")
        })
        .map(|(_, token)| token)
        .ok_or(Error::WrongRig)?;
    let hash = crate::auth_registry::hash_token(token);
    let client = service
        .clone()
        .query(move |store| store.client_for_token(&hash))
        .await?
        .ok_or(Error::WrongRig)?;
    if client.rig_id != rig
        || client.profile_id != sample.profile_id
        || profile(&headers)? != Some(sample.profile_id)
    {
        return Err(Error::WrongRig);
    }
    let now = now_ms();
    let answer = service
        .run(move |store| {
            let sample_id = sample.sample_id;
            let profile_id = sample.profile_id;
            let stored = store.record_weather(rig, sample, now)?;
            let weather = store.rig_weather(rig, Some(profile_id), now)?;
            Ok(WeatherAnswer {
                sample_id,
                profile_id,
                stored,
                weather,
            })
        })
        .await?;
    Ok(pairing::no_store(Json(ApiResponse::success(answer))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    from_ms: Option<u64>,
    to_ms: Option<u64>,
}

async fn history(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<ApiResponse<Vec<WeatherObservation>>>, Error> {
    let now = now_ms();
    let from = query.from_ms.unwrap_or(now.saturating_sub(24 * 3_600_000));
    let to = query.to_ms.unwrap_or(now);
    let rows = enabled(&state)?
        .query(move |store| store.weather_history(rig, from, to))
        .await?;
    Ok(Json(ApiResponse::success(rows)))
}
