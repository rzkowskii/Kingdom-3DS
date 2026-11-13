//! Building: a beggar hired, armed and building, and many builders on many
//! sites at once.

use game::data::Role;

use super::island;
use crate::scenario::{no_metrics, Area, Class, Scenario};
use crate::script::Script;
use crate::worlds::{self, proto};

pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "beggar_to_builder",
            area: Area::Construction,
            class: Class::Internal,
            summary:
                "A dropped coin hires a beggar, who takes a hammer and builds the wall scaffold",
            seed: 4,
            setup: |seed| {
                let mut s = island(seed, 5);
                worlds::place(&mut s.game, &s.world, proto::WALL1, -3.0, true);
                let shop = worlds::place(&mut s.game, &s.world, proto::SHOP_HAMMER, 3.0, false);
                worlds::spawn(&mut s.game, Role::Beggar, 1, 1.5, 0.0);
                s.game.population.stock_shop(&s.world, &s.game.level, shop);
                s
            },
            script: || Script::new().idle(0.5).tap(6).idle(45.0),
            checkpoint_every: 300,
            check: |s, c| {
                let roles: Vec<Role> = s.game.population.units.iter().map(|u| u.role).collect();
                c.eq("citizens", roles, vec![Role::Worker]);
                c.eq("promotions", s.log.count("Unit.Promoted"), 2);
                c.eq("walls built", s.log.count("Built"), 1);
                c.eq("wallet", s.game.wallet.coins(), 4);
            },
            metrics: no_metrics,
            known_issue: None,
        },
        Scenario {
            name: "many_builders",
            area: Area::Construction,
            class: Class::Internal,
            summary: "24 builders finish 40 big scaffolds (the Old 3DS stress case) before dusk",
            seed: 7,
            setup: |seed| {
                let mut s = island(seed, 0);
                for i in 0..40 {
                    let x = -20.0 + i as f32;
                    worlds::place(&mut s.game, &s.world, proto::SITE, x, true);
                }
                worlds::spawn(&mut s.game, Role::Worker, 24, -19.0, 1.6);
                s.closed_economy();
                s
            },
            script: || Script::new().idle(150.0),
            checkpoint_every: 600,
            check: |s, c| {
                c.eq("sites built", s.log.count("Built"), 40);
                c.eq("still under construction", s.final_under_construction(), 0);
            },
            metrics: no_metrics,
            known_issue: None,
        },
    ]
}
