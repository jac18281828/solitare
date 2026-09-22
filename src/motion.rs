//! Pure card-flight planning: which cards fly, from where, and on what
//! timing. No DOM access here — `main.rs` measures rectangles and drives
//! the browser; this module reasons about plain data only, so it runs in
//! `cargo test` on the host target.

use solitare::game::{Card, GameState, Suit};

/// A card's on-screen box in CSS pixels, as `getBoundingClientRect` reports
/// it: viewport-relative, matching a `position: fixed` flight layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A flight's duration and easing differ by why the card is moving: a
/// normal move travels briskly, a rejected drop settles back more gently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightKind {
    Travel,
    SettleBack,
}

impl FlightKind {
    pub fn duration_ms(self) -> f64 {
        match self {
            FlightKind::Travel => 220.0,
            FlightKind::SettleBack => 280.0,
        }
    }

    /// A flight always lands: every hidden card reappears no later than
    /// this deadline, even when no animation event ever fires.
    pub fn deadline_ms(self) -> f64 {
        self.duration_ms() + 100.0
    }
}

/// One card's journey from where it was last drawn to where it lands. `to`
/// is unset until the render that applies the move has let `main.rs`
/// measure the destination; until then the flight layer paints the card
/// motionless at `from`, matching the departure frame exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct Flight {
    pub card: Card,
    pub from: Rect,
    pub to: Option<Rect>,
    pub kind: FlightKind,
    pub launched_at: f64,
    /// The flight's very first launch, held constant across every re-aim:
    /// the anchor for its landing ceiling, since `launched_at` itself moves
    /// forward on each re-aim.
    pub first_launched_at: f64,
    /// Other cards that arrived in the same pile in the same batch without
    /// a flight of their own (a hard draw's lower two cards): excluded
    /// from underlay consideration until this flight lands.
    pub covers: Vec<Card>,
}

impl Flight {
    pub fn new(card: Card, from: Rect, kind: FlightKind, launched_at: f64) -> Self {
        Self {
            card,
            from,
            to: None,
            kind,
            launched_at,
            first_launched_at: launched_at,
            covers: Vec::new(),
        }
    }

    pub fn with_covers(mut self, covers: Vec<Card>) -> Self {
        self.covers = covers;
        self
    }

    /// The absolute latest moment this flight may still be in the air,
    /// however many times it has been re-aimed: its first launch plus
    /// twice its duration, with the same 100ms landing grace as a normal
    /// deadline. Without a ceiling, a destination that keeps moving could
    /// re-aim — and so postpone — a flight's landing forever.
    fn ceiling(&self) -> f64 {
        self.first_launched_at + 2.0 * self.kind.duration_ms() + 100.0
    }

    pub fn deadline(&self) -> f64 {
        (self.launched_at + self.kind.deadline_ms()).min(self.ceiling())
    }

    pub fn has_expired(&self, now: f64) -> bool {
        now >= self.deadline()
    }
}

/// Re-aims a flight already under way: continues it from its current
/// on-screen point toward a newly measured destination, restarting its
/// clock from this moment so its deadline counts from the re-aim rather
/// than the flight's original launch — up to the flight's ceiling.
pub fn reaim(flight: &mut Flight, live: Rect, destination: Rect, now: f64) {
    flight.from = live;
    flight.to = Some(destination);
    flight.launched_at = now;
}

/// What a flight's fresh measurement calls for: `destination` and `live`
/// are whatever `main.rs` found (or did not) for the flight's card at its
/// board slot and its own flight-layer element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlightMeasurement {
    /// The card has no element anywhere (buried by a later draw before this
    /// flight could land): it lands at once rather than hanging until its
    /// deadline.
    Vanished,
    /// The destination moved since the flight last aimed at it: re-aim
    /// toward it from the flight's current live position.
    Moved { live: Rect, destination: Rect },
    /// Nothing to do: the destination is unmeasured, unchanged, or the
    /// flight's own live position could not be read this frame.
    Unchanged,
}

/// Resolves one flight's measurement, given what `main.rs` found (or did
/// not) in the DOM this frame. Ctx-free and DOM-free, so a host test can
/// drive it directly.
pub fn resolve_flight_measurement(
    flight: &Flight,
    destination: Option<Rect>,
    live: Option<Rect>,
) -> FlightMeasurement {
    let Some(destination) = destination else {
        return FlightMeasurement::Vanished;
    };
    if flight.to == Some(destination) {
        return FlightMeasurement::Unchanged;
    }
    match live {
        Some(live) => FlightMeasurement::Moved { live, destination },
        None => FlightMeasurement::Unchanged,
    }
}

/// A card's departure point: its live in-flight position when one exists,
/// otherwise its board (or drag-overlay) position. A card moved again
/// mid-flight continues from where it is rather than snapping back to a
/// stale slot.
pub fn departure_rect(board: Rect, in_flight: Option<Rect>) -> Rect {
    in_flight.unwrap_or(board)
}

/// Where a face-up card sits, coarse enough to detect a flight-worthy
/// move: which pile, not its position within it. Appending to or removing
/// from the top of a pile never reflows the cards below it, so pile
/// identity alone is enough to tell a relocation from a re-fan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Waste,
    Foundation(usize),
    Tableau(usize),
}

/// Every face-up card's slot: the input a move's before/after diff reads.
pub type Snapshot = Vec<(Card, Slot)>;

pub fn snapshot(game: &GameState) -> Snapshot {
    let mut slots = Snapshot::new();
    if let Some(card) = game.waste.last() {
        slots.push((*card, Slot::Waste));
    }
    for (pile, cards) in game.foundations.iter().enumerate() {
        if let Some(card) = cards.last() {
            slots.push((*card, Slot::Foundation(pile)));
        }
    }
    for (pile, cards) in game.tableau.iter().enumerate() {
        for tableau_card in cards {
            if tableau_card.face_up {
                slots.push((tableau_card.card, Slot::Tableau(pile)));
            }
        }
    }
    slots
}

fn slot_of(snapshot: &Snapshot, card: Card) -> Option<Slot> {
    snapshot
        .iter()
        .find(|(c, _)| *c == card)
        .map(|(_, slot)| *slot)
}

/// Cards whose slot changed between two snapshots — relocated to a
/// different pile, not merely re-fanned in place. A card absent from
/// `before` (a face-down card just turned up, or one dealt fresh) is
/// excluded: it appears in place, it does not fly.
pub fn moved_cards(before: &Snapshot, after: &Snapshot) -> Vec<Card> {
    after
        .iter()
        .filter_map(|(card, slot)| match slot_of(before, *card) {
            Some(previous) if previous != *slot => Some(*card),
            _ => None,
        })
        .collect()
}

/// Plans flights for a move already applied to `GameState`: every card the
/// snapshot diff calls moved, launched from its captured departure rect.
/// Empty under reduced motion. A moved card missing from `departures`
/// (its rect could not be measured) simply gets no flight — it still
/// lands correctly, just without the animation.
pub fn plan_flights_for_move(
    before: &Snapshot,
    after: &Snapshot,
    departures: &[(Card, Rect)],
    kind: FlightKind,
    reduced_motion: bool,
    launched_at: f64,
) -> Vec<Flight> {
    if reduced_motion {
        return Vec::new();
    }
    moved_cards(before, after)
        .into_iter()
        .filter_map(|card| {
            departures
                .iter()
                .find(|(c, _)| *c == card)
                .map(|(_, rect)| Flight::new(card, *rect, kind, launched_at))
        })
        .collect()
}

/// The flights a drag's release plans: every dragged card whose departure
/// was captured flies from there — `Travel` on a landed drop, `SettleBack`
/// on a rejected one or a cancel. A card missing from `departures` (its
/// rect could not be measured) simply gets no flight.
pub fn plan_drag_flights(
    dragged: &[Card],
    departures: &[(Card, Rect)],
    kind: FlightKind,
    launched_at: f64,
) -> Vec<Flight> {
    dragged
        .iter()
        .filter_map(|card| {
            departures
                .iter()
                .find(|(c, _)| c == card)
                .map(|(_, rect)| Flight::new(*card, *rect, kind, launched_at))
        })
        .collect()
}

/// A draw's new waste top departs from the stock slot, already face up —
/// the one exception to snapshot diffing, since a card fresh off the stock
/// never had a prior face-up slot to compare against. `None` on a recycle
/// (no new top) or under reduced motion.
pub fn plan_draw_flight(
    new_top: Option<Card>,
    stock_slot: Rect,
    reduced_motion: bool,
    launched_at: f64,
) -> Option<Flight> {
    if reduced_motion {
        return None;
    }
    let new_top = new_top?;
    Some(Flight::new(
        new_top,
        stock_slot,
        FlightKind::Travel,
        launched_at,
    ))
}

/// The card an underlay should show beneath a pile's flying or lifted top:
/// the highest remaining card that is neither itself flying nor a silent
/// sibling (`Flight::covers`) of a flight yet to land. `pile` excludes the
/// top card itself — the caller already knows it is away.
pub fn underlay_card(pile: &[Card], flights: &[Flight]) -> Option<Card> {
    let mut index = pile.len();
    while index > 0 {
        index -= 1;
        let candidate = pile[index];
        if let Some(flight) = flights.iter().find(|f| f.card == candidate) {
            // The candidate is itself flying: also skip the silent
            // siblings that arrived beneath it in the same batch.
            let skip = flight.covers.len();
            if index < skip {
                return None;
            }
            index -= skip;
            continue;
        }
        if flights.iter().any(|f| f.covers.contains(&candidate)) {
            // A silent sibling of some other flight, carried in but never
            // independently visible until that flight lands.
            continue;
        }
        return Some(candidate);
    }
    None
}

/// A card's stable identity string ("7H", "10S"), used as a DOM attribute
/// on face-up cards and as a Yew list key everywhere. Not sensitive: it is
/// only ever attached to a card the player can already see.
pub fn card_key(card: Card) -> String {
    format!("{}{}", card.rank, suit_letter(card.suit))
}

fn suit_letter(suit: Suit) -> char {
    match suit {
        Suit::Clubs => 'C',
        Suit::Diamonds => 'D',
        Suit::Hearts => 'H',
        Suit::Spades => 'S',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spade(rank: u8) -> Card {
        Card {
            suit: Suit::Spades,
            rank,
        }
    }

    fn heart(rank: u8) -> Card {
        Card {
            suit: Suit::Hearts,
            rank,
        }
    }

    fn rect(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            width: 60.0,
            height: 85.0,
        }
    }

    #[test]
    fn travel_duration_and_deadline() {
        assert_eq!(FlightKind::Travel.duration_ms(), 220.0);
        assert_eq!(FlightKind::Travel.deadline_ms(), 320.0);
    }

    #[test]
    fn settle_back_duration_and_deadline() {
        assert_eq!(FlightKind::SettleBack.duration_ms(), 280.0);
        assert_eq!(FlightKind::SettleBack.deadline_ms(), 380.0);
    }

    #[test]
    fn a_flights_deadline_is_its_duration_plus_100ms() {
        let flight = Flight::new(spade(5), rect(0.0, 0.0), FlightKind::Travel, 1_000.0);
        assert_eq!(flight.deadline(), 1_000.0 + 320.0);
        assert!(!flight.has_expired(1_319.0));
        assert!(flight.has_expired(1_320.0));
    }

    #[test]
    fn departure_prefers_the_in_flight_position() {
        let board = rect(0.0, 0.0);
        let in_flight = rect(40.0, 12.0);
        assert_eq!(departure_rect(board, Some(in_flight)), in_flight);
        assert_eq!(departure_rect(board, None), board);
    }

    #[test]
    fn a_flight_with_no_destination_lands_at_once() {
        let flight = Flight::new(spade(5), rect(0.0, 0.0), FlightKind::Travel, 0.0);

        let measurement = resolve_flight_measurement(&flight, None, None);

        assert_eq!(measurement, FlightMeasurement::Vanished);
    }

    #[test]
    fn a_flight_re_aims_toward_a_moved_destination() {
        let mut flight = Flight::new(spade(5), rect(0.0, 0.0), FlightKind::Travel, 0.0);
        flight.to = Some(rect(10.0, 10.0));

        let measurement =
            resolve_flight_measurement(&flight, Some(rect(20.0, 20.0)), Some(rect(5.0, 5.0)));

        assert_eq!(
            measurement,
            FlightMeasurement::Moved {
                live: rect(5.0, 5.0),
                destination: rect(20.0, 20.0),
            }
        );
    }

    #[test]
    fn reaiming_a_flight_resets_its_clock_to_the_reaim_moment() {
        let mut flight = Flight::new(spade(5), rect(0.0, 0.0), FlightKind::Travel, 100.0);
        flight.to = Some(rect(50.0, 50.0));

        // Well short of the ceiling (100.0 + 2 * 220.0 + 100.0 = 640.0), so
        // the normal per-re-aim deadline governs, not the ceiling.
        reaim(&mut flight, rect(20.0, 20.0), rect(90.0, 90.0), 200.0);

        assert_eq!(flight.from, rect(20.0, 20.0));
        assert_eq!(flight.to, Some(rect(90.0, 90.0)));
        assert_eq!(flight.launched_at, 200.0);
        assert_eq!(flight.deadline(), 200.0 + FlightKind::Travel.deadline_ms());
    }

    #[test]
    fn reaiming_never_moves_the_landing_past_the_ceiling() {
        // First launch at 0ms; the ceiling is 2 * 220ms + 100ms = 540ms from
        // there, however often the flight is re-aimed.
        let mut flight = Flight::new(spade(5), rect(0.0, 0.0), FlightKind::Travel, 0.0);

        reaim(&mut flight, rect(1.0, 1.0), rect(2.0, 2.0), 500.0);

        assert_eq!(flight.deadline(), 540.0);
    }

    #[test]
    fn a_relocated_card_flies_and_a_card_left_in_place_does_not() {
        let before = vec![(spade(5), Slot::Tableau(0)), (heart(9), Slot::Tableau(1))];
        let after = vec![(spade(5), Slot::Tableau(2)), (heart(9), Slot::Tableau(1))];

        let moved = moved_cards(&before, &after);

        assert_eq!(moved, vec![spade(5)]);
    }

    #[test]
    fn a_newly_exposed_card_appears_in_place() {
        // heart(9) was face down (absent from `before`); zeus_vision or a
        // flip turns it face up without moving anything.
        let before = vec![(spade(5), Slot::Tableau(0))];
        let after = vec![(spade(5), Slot::Tableau(0)), (heart(9), Slot::Tableau(1))];

        assert!(moved_cards(&before, &after).is_empty());
    }

    #[test]
    fn plan_flights_for_move_launches_only_the_moved_card() {
        let before = vec![(spade(5), Slot::Waste)];
        let after = vec![(spade(5), Slot::Foundation(0))];
        let departures = vec![(spade(5), rect(10.0, 20.0))];

        let flights =
            plan_flights_for_move(&before, &after, &departures, FlightKind::Travel, false, 0.0);

        assert_eq!(flights.len(), 1);
        assert_eq!(flights[0].card, spade(5));
        assert_eq!(flights[0].from, rect(10.0, 20.0));
        assert!(flights[0].to.is_none());
    }

    #[test]
    fn reduced_motion_launches_nothing() {
        let before = vec![(spade(5), Slot::Waste)];
        let after = vec![(spade(5), Slot::Foundation(0))];
        let departures = vec![(spade(5), rect(10.0, 20.0))];

        let flights =
            plan_flights_for_move(&before, &after, &departures, FlightKind::Travel, true, 0.0);

        assert!(flights.is_empty());
    }

    #[test]
    fn plan_drag_flights_uses_the_requested_kind_for_every_dragged_card() {
        let dragged = vec![spade(5), heart(6)];
        let departures = vec![(spade(5), rect(1.0, 2.0)), (heart(6), rect(3.0, 4.0))];

        let flights = plan_drag_flights(&dragged, &departures, FlightKind::SettleBack, 0.0);

        assert_eq!(flights.len(), 2);
        assert!(flights.iter().all(|f| f.kind == FlightKind::SettleBack));
        assert_eq!(
            flights.iter().find(|f| f.card == spade(5)).unwrap().from,
            rect(1.0, 2.0)
        );
    }

    #[test]
    fn plan_drag_flights_skips_a_card_with_no_captured_departure() {
        let dragged = vec![spade(5)];
        let flights = plan_drag_flights(&dragged, &[], FlightKind::Travel, 0.0);

        assert!(flights.is_empty());
    }

    #[test]
    fn a_draw_departs_from_the_stock_slot() {
        let stock = rect(4.0, 8.0);
        let flight = plan_draw_flight(Some(spade(1)), stock, false, 0.0).expect("a card drew");

        assert_eq!(flight.card, spade(1));
        assert_eq!(flight.from, stock);
        assert_eq!(flight.kind, FlightKind::Travel);
    }

    #[test]
    fn a_recycle_launches_nothing() {
        // A recycle empties the waste: there is no new top to draw.
        assert!(plan_draw_flight(None, rect(0.0, 0.0), false, 0.0).is_none());
    }

    #[test]
    fn reduced_motion_launches_no_draw_flight() {
        assert!(plan_draw_flight(Some(spade(1)), rect(0.0, 0.0), true, 0.0).is_none());
    }

    #[test]
    fn underlay_shows_the_card_beneath_a_flying_top() {
        let pile = vec![spade(5), heart(9)];
        let flights = Vec::new();

        assert_eq!(underlay_card(&pile, &flights), Some(heart(9)));
    }

    #[test]
    fn underlay_skips_a_hard_draws_silent_siblings() {
        // Waste after a hard draw of three: only the true top (spade(1))
        // gets a flight, which covers the two silent cards beneath it.
        let old_top = heart(9);
        let silent = spade(2);
        let pile = vec![old_top, silent];
        let flying_top = spade(1);
        let flight = Flight::new(flying_top, rect(0.0, 0.0), FlightKind::Travel, 0.0)
            .with_covers(vec![silent]);

        assert_eq!(underlay_card(&pile, &[flight]), Some(old_top));
    }

    #[test]
    fn underlay_is_none_when_everything_beneath_is_covered() {
        let silent = spade(2);
        let pile = vec![silent];
        let flying_top = spade(1);
        let flight = Flight::new(flying_top, rect(0.0, 0.0), FlightKind::Travel, 0.0)
            .with_covers(vec![silent]);

        assert_eq!(underlay_card(&pile, &[flight]), None);
    }

    #[test]
    fn underlay_walks_past_a_second_flying_card() {
        // All To Temple stacks two promotions on one foundation 110ms
        // apart: the lower card may still be flying when the upper one
        // launches, so neither may show as the other's underlay.
        let settled = heart(2);
        let lower_flying = spade(3);
        let pile = vec![settled, lower_flying];
        let flights = vec![Flight::new(
            lower_flying,
            rect(0.0, 0.0),
            FlightKind::Travel,
            0.0,
        )];

        assert_eq!(underlay_card(&pile, &flights), Some(settled));
    }

    #[test]
    fn card_key_pairs_rank_with_suit_letter() {
        assert_eq!(card_key(spade(1)), "1S");
        assert_eq!(card_key(heart(10)), "10H");
    }
}
