
use std::io::{self, IsTerminal};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {

    Kitty,

    Iterm2,

    Sixel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Support {

    pub tty: bool,
    pub kitty: bool,
    pub iterm2: bool,
    pub sixel: bool,
}

fn choose(support: Support) -> Option<ImageProtocol> {
    if !support.tty {
        return None;
    }
    if support.kitty {
        return Some(ImageProtocol::Kitty);
    }
    if support.iterm2 {
        return Some(ImageProtocol::Iterm2);
    }
    if support.sixel {
        return Some(ImageProtocol::Sixel);
    }
    None
}

fn probe() -> Support {

    if !io::stdout().is_terminal() {
        return Support::default();
    }

    Support {
        tty: true,
        kitty: viuer::get_kitty_support() != viuer::KittySupport::None,
        iterm2: viuer::is_iterm_supported(),
        sixel: viuer::is_sixel_supported(),
    }
}

pub fn protocol() -> Option<ImageProtocol> {
    static DETECTED: OnceLock<Option<ImageProtocol>> = OnceLock::new();
    *DETECTED.get_or_init(|| choose(probe()))
}

pub fn supported() -> bool {
    protocol().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_with_nothing_to_offer_gets_no_protocol() {
        assert_eq!(
            choose(Support {
                tty: true,
                ..Support::default()
            }),
            None
        );
    }

    #[test]
    fn piped_output_is_never_drawable_however_capable_the_terminal_is() {

        assert_eq!(
            choose(Support {
                tty: false,
                kitty: true,
                iterm2: true,
                sixel: true,
            }),
            None
        );
    }

    #[test]
    fn each_protocol_is_picked_when_it_is_the_only_one() {
        for (support, expected) in [
            (
                Support {
                    tty: true,
                    kitty: true,
                    ..Support::default()
                },
                ImageProtocol::Kitty,
            ),
            (
                Support {
                    tty: true,
                    iterm2: true,
                    ..Support::default()
                },
                ImageProtocol::Iterm2,
            ),
            (
                Support {
                    tty: true,
                    sixel: true,
                    ..Support::default()
                },
                ImageProtocol::Sixel,
            ),
        ] {
            assert_eq!(choose(support), Some(expected), "{support:?}");
        }
    }

    #[test]
    fn a_terminal_that_speaks_several_protocols_gets_the_best_one() {

        assert_eq!(
            choose(Support {
                tty: true,
                kitty: true,
                iterm2: false,
                sixel: true,
            }),
            Some(ImageProtocol::Kitty)
        );
        assert_eq!(
            choose(Support {
                tty: true,
                kitty: false,
                iterm2: true,
                sixel: true,
            }),
            Some(ImageProtocol::Iterm2)
        );
    }
}
