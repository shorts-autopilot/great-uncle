//! Render benchmarks: the numbers an implementer checks by hand after changing
//! the renderer, a fade lane or the effect layer.
//!
//! There is no timing assertion anywhere. A benchmark that fails a build on a
//! busy machine is worse than no benchmark; the record in `docs/PHASE5.md` is
//! the point.
//!
//! A 512-slot universe cannot hold 4096 one-channel fixtures across four
//! universes, so the large rig here is 2048 fixtures — the physical maximum for
//! four universes at one slot each.

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use engine::fixture_type::{AttributeDef, Channel, ChannelFunction};
use engine::playback::{Cue, Sequence};
use engine::{Engine, Fixture, FixtureType, Levels, Mode, Show, Target, Universe};

fn attribute(name: &str) -> AttributeDef {
    AttributeDef {
        name: name.into(),
        pretty: name.into(),
        feature: name.into(),
        unit: String::new(),
    }
}

fn channel(offset: u16, name: &str) -> Channel {
    Channel {
        offset,
        fine_offset: None,
        attribute: name.into(),
        default_raw: 0,
        functions: vec![ChannelFunction {
            name: name.into(),
            dmx_from: 0,
            dmx_to: 255,
            physical_from: 0.0,
            physical_to: 1.0,
        }],
    }
}

/// A show with `fixtures` fixtures, each carrying `attributes` channels, spread
/// evenly over as many 512-slot universes as it needs.
fn show_of(fixtures: u16, attributes: &[&str]) -> Show {
    let per_universe = (512 / attributes.len().max(1)) as u16;
    let universes = fixtures.div_ceil(per_universe);
    let mut show = Show::new();
    show.name = "Bench".into();
    show.universes = (1..=universes)
        .map(|number| Universe {
            number,
            label: format!("Universe {number}"),
            size: 512,
        })
        .collect();
    show.fixture_types.push(FixtureType {
        manufacturer: "OpenLight".into(),
        name: "Bench Unit".into(),
        description: String::new(),
        attributes: attributes.iter().map(|name| attribute(name)).collect(),
        modes: vec![Mode {
            name: format!("{}ch", attributes.len()),
            channels: attributes
                .iter()
                .enumerate()
                .map(|(index, name)| channel(index as u16 + 1, name))
                .collect(),
        }],
        home: Default::default(),
    });
    show.fixtures = (1..=fixtures)
        .map(|number| {
            let index = number - 1;
            Fixture {
                number,
                name: format!("Bench {number}"),
                type_key: "OpenLight Bench Unit".into(),
                mode: format!("{}ch", attributes.len()),
                universe: index / per_universe + 1,
                address: index % per_universe * attributes.len() as u16 + 1,
                home: Default::default(),
            }
        })
        .collect();
    show
}

fn programmer_engine(fixtures: u16) -> Engine {
    let show = show_of(fixtures, &["Dimmer"]);
    let mut engine = Engine::new(show);
    for number in 1..=fixtures {
        engine.desk.programmer.set(number, "Dimmer", 0.75);
    }
    engine
}

fn playback_engine(fixtures: u16, lanes: u16) -> Engine {
    let mut show = show_of(fixtures, &["Dimmer"]);
    let mut levels = Levels::new();
    for number in 1..=lanes {
        levels.set(number, "Dimmer", 0.8);
    }
    let mut sequence = Sequence::new(1);
    sequence.cues.push(Cue {
        number: 1.0,
        name: "Bench".into(),
        fade: 0.0,
        delay: 0.0,
        mib: false,
        trigger: Default::default(),
        trigger_time: 0.0,
        timing: Default::default(),
        stagger: 0.0,
        blocked: true,
        shield: Default::default(),
        levels,
    });
    show.sequences.push(sequence);
    let mut engine = Engine::new(show);
    let now = engine.clock();
    engine
        .playback
        .go(&engine.show, Target::Sequence(1), 1, 0, now)
        .expect("the bench cue starts");
    engine.tick(now + Duration::from_secs(1));
    engine
}

fn effect_engine(fixtures: u16) -> Engine {
    let show = show_of(fixtures, &["Dimmer", "Pan", "Tilt"]);
    let mut engine = Engine::new(show);
    for line in [
        format!("Fixture 1 Thru {fixtures}"),
        "Store Effect 1 \"Bench\" Attributes Pan Tilt Dimmer".into(),
        "Effect 1 On".into(),
    ] {
        engine.execute(&line).expect("bench effect setup");
    }
    engine.tick(Instant::now() + Duration::from_millis(16));
    engine
}

fn renders(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("render");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(3));

    group.bench_with_input(
        BenchmarkId::new("programmer", "2048 fixtures / 4 universes"),
        &2048,
        |bencher, fixtures| {
            let engine = programmer_engine(*fixtures);
            bencher.iter(|| engine.render());
        },
    );

    group.bench_with_input(
        BenchmarkId::new("playback", "1000 lanes"),
        &1000,
        |bencher, lanes| {
            let engine = playback_engine(2048, *lanes);
            bencher.iter(|| engine.render());
        },
    );

    group.bench_with_input(
        BenchmarkId::new("effect", "512 fixtures / 3 targets"),
        &512,
        |bencher, fixtures| {
            let engine = effect_engine(*fixtures);
            bencher.iter(|| engine.render());
        },
    );

    group.finish();
}

criterion_group!(benches, renders);
criterion_main!(benches);
