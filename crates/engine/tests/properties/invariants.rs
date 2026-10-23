use engine::Engine;
use proptest::prelude::*;

use crate::strategy::commands;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3))]

    #[test]
    fn print_generated(cmds in commands()) {
        let mut engine = Engine::new();
        println!("--- case ---");
        for cmd in cmds {
            let events = engine.apply(cmd);
            println!("{cmd:?}\n    -> {events:?}");
        }
    }
}
