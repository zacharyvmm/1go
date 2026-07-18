use criterion::Criterion;
use std::time::Duration;

pub fn criterion_config() -> Criterion {
    match std::env::var("SCAH_BENCH_PROFILE").as_deref() {
        Ok("quick") => Criterion::default()
            .sample_size(30)
            .warm_up_time(Duration::from_secs(1))
            .measurement_time(Duration::from_secs(2)),

        Ok("full") | Err(_) => Criterion::default()
            .sample_size(100)
            .warm_up_time(Duration::from_secs(3))
            .measurement_time(Duration::from_secs(5)),

        Ok(profile) => {
            panic!("unsupported SCAH_BENCH_PROFILE={profile:?}; expected `quick` or `full`");
        }
    }
}
