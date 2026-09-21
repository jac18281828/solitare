mod motion;

use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use log::info;
use motion::{Flight, FlightKind, Rect};
use solitare::game::{Card, EASY_DRAW_COUNT, GameState, HARD_DRAW_COUNT, Selection, TableauCard};
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent as DomKeyboardEvent;
use yew::events::{MouseEvent, PointerEvent};
use yew::{Classes, Component, Context, Html, Renderer, classes, html};

const TEMPLE_GOLD_STORAGE_KEY: &str = "solitare.temple_gold";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load_temple_gold() -> usize {
    local_storage()
        .and_then(|s| s.get_item(TEMPLE_GOLD_STORAGE_KEY).ok().flatten())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn persist_temple_gold(value: usize) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(TEMPLE_GOLD_STORAGE_KEY, &value.to_string());
    }
}

/// A card's face-down and face-up step counts within its tableau column,
/// counting only the cards beneath it (dealt before it). The stylesheet
/// resolves these into fan offsets and pile height via `--fan-down`/
/// `--fan-up`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CardSteps {
    down: usize,
    up: usize,
}

/// Per-card step counts for a tableau column, plus the same counts for the
/// whole pile: the last card's steps, so the pile sizes to that card's
/// offset rather than one step past it. An empty column has zero steps.
struct TableauFan {
    cards: Vec<CardSteps>,
    pile: CardSteps,
}

fn fan_offsets(pile: &[TableauCard]) -> TableauFan {
    let mut down = 0usize;
    let mut up = 0usize;
    let cards: Vec<CardSteps> = pile
        .iter()
        .map(|card| {
            let steps = CardSteps { down, up };
            if card.face_up {
                up += 1;
            } else {
                down += 1;
            }
            steps
        })
        .collect();
    let pile_steps = cards.last().copied().unwrap_or_default();
    TableauFan {
        cards,
        pile: pile_steps,
    }
}

/// A point in CSS pixels from a pointer event's `clientX`/`clientY`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPoint {
    x: f64,
    y: f64,
}

/// A pointer that travels this far or less between `pointerdown` and
/// `pointerup` is a tap and must produce exactly the click behavior for its
/// element; past it, the gesture is a drag.
const TAP_THRESHOLD_PX: f64 = 3.0;

fn exceeds_tap_threshold(start: PointerPoint, current: PointerPoint) -> bool {
    (current.x - start.x).hypot(current.y - start.y) > TAP_THRESHOLD_PX
}

/// A gesture stays a drag for the rest of its life once it crosses the tap
/// threshold, even if the pointer drifts back within range of `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragPhase {
    Pressed,
    Dragging,
}

fn advance_drag_phase(phase: DragPhase, start: PointerPoint, current: PointerPoint) -> DragPhase {
    if phase == DragPhase::Dragging || exceeds_tap_threshold(start, current) {
        DragPhase::Dragging
    } else {
        DragPhase::Pressed
    }
}

/// A destination a drop can land on, resolved by hit-testing the DOM under
/// the pointer (`App::hit_test_drop_target`). Never built in a unit test —
/// browser behavior is proven in the drag-input prompt's §8, not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropTarget {
    Tableau(usize),
    Foundation(usize),
}

/// The result of resolving a drag's drop against its target: whether the
/// move landed, and the status line to show — worded identically to the
/// equivalent two-tap move (§8).
#[derive(Clone, Debug, PartialEq, Eq)]
struct DropOutcome {
    moved: bool,
    status: String,
}

/// One pointer's press-to-release gesture on a card or run. `origin` is the
/// selection the gesture would pick up; `selection_before` is whatever was
/// selected before the gesture started, restored on cancel or an illegal
/// drop so a failed gesture leaves no trace. `grab_offset` is the press
/// point's offset from the pressed card's own top-left corner, so the
/// overlay can keep that exact point under the pointer instead of
/// centering the card on it.
#[derive(Clone, Debug, PartialEq)]
struct DragTracker {
    pointer_id: i32,
    origin: Selection,
    selection_before: Option<Selection>,
    start: PointerPoint,
    current: PointerPoint,
    grab_offset: PointerPoint,
    phase: DragPhase,
}

impl DragTracker {
    fn new(
        pointer_id: i32,
        origin: Selection,
        selection_before: Option<Selection>,
        at: PointerPoint,
        grab_offset: PointerPoint,
    ) -> Self {
        Self {
            pointer_id,
            origin,
            selection_before,
            start: at,
            current: at,
            grab_offset,
            phase: DragPhase::Pressed,
        }
    }

    fn moved(&self, at: PointerPoint) -> Self {
        Self {
            pointer_id: self.pointer_id,
            origin: self.origin,
            selection_before: self.selection_before,
            start: self.start,
            current: at,
            grab_offset: self.grab_offset,
            phase: advance_drag_phase(self.phase, self.start, at),
        }
    }
}

/// The four pointer callbacks a draggable card wires onto its button,
/// bundled so `view_face_card` takes one argument instead of four.
struct PointerCallbacks {
    down: yew::Callback<PointerEvent>,
    move_: yew::Callback<PointerEvent>,
    up: yew::Callback<PointerEvent>,
    cancel: yew::Callback<PointerEvent>,
}

/// Ties every later pointer event in this gesture to the element under the
/// initial press, so the drag survives the pointer leaving the card's box.
fn capture_pointer(event: &PointerEvent) {
    if let Some(target) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        let _ = target.set_pointer_capture(event.pointer_id());
    }
}

/// Releases capture on `pointerup` and `pointercancel`; safe to call even
/// if the browser already dropped capture on its own.
fn release_pointer(event: &PointerEvent) {
    if let Some(target) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        let _ = target.release_pointer_capture(event.pointer_id());
    }
}

/// The OS motion preference, read fresh at every launch so a change takes
/// effect without a reload rather than only at the next page load.
fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|query| query.matches())
}

/// An element's current on-screen box, read live from the DOM — reflects a
/// card's true position even mid-transition, since `getBoundingClientRect`
/// always reports the browser's current computed geometry.
fn element_rect(selector: &str) -> Option<Rect> {
    let element = web_sys::window()?
        .document()?
        .query_selector(selector)
        .ok()??;
    let rect = element.get_bounding_client_rect();
    Some(Rect {
        x: rect.x(),
        y: rect.y(),
        width: rect.width(),
        height: rect.height(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndState {
    ZeusThunder,
    OutOfGold,
    Victory,
    Stalemate,
}

impl EndState {
    fn is_loss(self) -> bool {
        matches!(self, Self::ZeusThunder | Self::OutOfGold | Self::Stalemate)
    }
}

/// Status text the click and drag paths must agree on word for word (§8).
const ILLEGAL_TABLEAU_MOVE: &str = "Illegal tableau move.";
const ILLEGAL_FOUNDATION_MOVE: &str = "Illegal foundation move.";
const EMPTY_TABLEAU_NEEDS_KING: &str = "Only a King can move into an empty tableau column.";

pub struct App {
    game: GameState,
    status: String,
    end_state: Option<EndState>,
    help_expanded: bool,
    victory_gold_award: usize,
    victory_rain_dismissed: bool,
    all_to_temple_running: bool,
    all_to_temple_timeout: Option<Timeout>,
    key_listener: Option<EventListener>,
    /// The pointer gesture in progress, if any; `None` outside a press.
    drag: Option<DragTracker>,
    /// Set when a drag resolves at `pointerup`, so the trailing synthetic
    /// click (and, for the tap-then-drag sequence, a trailing dblclick) is
    /// swallowed instead of re-running the click model on the same input.
    /// `schedule_just_dragged_clear` clears it on the next macrotask, after
    /// that same-tick trailing burst has run; a guarded handler only
    /// consults the flag, it never clears it, or the second of a paired
    /// click+dblclick would see it already gone.
    just_dragged: bool,
    /// The drop destination currently under the pointer, for highlighting.
    hover_target: Option<DropTarget>,
    /// Every card currently travelling from where it was last drawn to
    /// where it lands, or settling back from a rejected drop.
    flights: Vec<Flight>,
}

pub enum Msg {
    Noop,
    NewGame,
    DrawStock,
    ClickWaste,
    DoubleClickWaste,
    ClickFoundation(usize),
    ClickTableauCard(usize, usize),
    DoubleClickTableauCard(usize, usize),
    ClickTableauPile(usize),
    AutoFoundation,
    AllToTemple,
    AllToTempleStep,
    ZeusVision,
    SwitchDrawMode,
    ToggleHelp,
    DismissVictoryRain,
    PointerDown(Selection, i32, PointerPoint, PointerPoint),
    PointerMove(i32, f64, f64),
    PointerUp(i32, f64, f64),
    PointerCancel(i32),
    ClearJustDragged,
    /// A flight's destination, measured once its board copy has rendered.
    /// `f64` is the flight's `launched_at`, guarding against a flight that
    /// was replaced between the measurement and this message's delivery.
    FlightDestinationsMeasured(Vec<(Card, f64, Rect)>),
    /// A flight's fallback landing: fires unconditionally at its deadline
    /// so a hidden card always reappears, even without an animation event.
    ExpireFlight(Card, f64),
}

impl App {
    fn interactions_locked(&self) -> bool {
        self.end_state.is_some() || self.all_to_temple_running
    }

    fn describe_card(card: Card) -> String {
        format!("{}{}", card.rank_label(), card.suit.symbol())
    }

    fn tableau_move_status(pile: usize) -> String {
        format!("Moved cards to tableau column {}.", pile + 1)
    }

    fn foundation_move_status(pile: usize) -> String {
        format!("Placed card on foundation {}.", pile + 1)
    }

    /// Words a rejected move onto `pile`: only an empty column needs a King,
    /// every other rejection is an illegal move. Shared by the click and
    /// drag paths so they cannot reword the same rejection differently.
    fn tableau_rejection_status(&self, pile: usize) -> String {
        if self.game.tableau[pile].is_empty() {
            EMPTY_TABLEAU_NEEDS_KING.to_string()
        } else {
            ILLEGAL_TABLEAU_MOVE.to_string()
        }
    }

    /// Clears `just_dragged` on the next macrotask. Yew drains every message
    /// queued within one JS task — including a trailing click and, when one
    /// follows, its paired dblclick — before this timer's task can run, so
    /// the guard survives exactly that trailing burst and never strands past
    /// it into an unrelated later click.
    fn schedule_just_dragged_clear(ctx: &Context<Self>) {
        let link = ctx.link().clone();
        Timeout::new(0, move || link.send_message(Msg::ClearJustDragged)).forget();
    }

    fn schedule_all_to_temple_step(&mut self, ctx: &Context<Self>) {
        let link = ctx.link().clone();
        self.all_to_temple_timeout = Some(Timeout::new(110, move || {
            link.send_message(Msg::AllToTempleStep);
        }));
    }

    fn stop_all_to_temple(&mut self) {
        self.all_to_temple_running = false;
        self.all_to_temple_timeout = None;
    }

    fn trigger_victory(&mut self) {
        if matches!(self.end_state, Some(EndState::Victory)) {
            return;
        }

        let reward = self.game.temple_gold;
        self.victory_gold_award = reward;
        self.end_state = Some(EndState::Victory);
        self.stop_all_to_temple();
        self.status = format!("Dionysus honors you with {reward} gold.");
    }

    /// Builds the pointer handlers a draggable card wires onto its button.
    /// Capture/release happen here, at the moment the raw event is in hand;
    /// `update` only ever sees the extracted coordinates.
    fn pointer_callbacks(ctx: &Context<Self>, origin: Selection) -> PointerCallbacks {
        let link = ctx.link();
        let down = link.callback(move |event: PointerEvent| {
            capture_pointer(&event);
            let at = PointerPoint {
                x: event.client_x() as f64,
                y: event.client_y() as f64,
            };
            // The one measurement PointerDown may take: the pressed card's
            // own rect, so the overlay can keep this exact point under the
            // pointer instead of centering the card on it. `target()`, not
            // `current_target()` — Yew delegates events to a shared root,
            // so `current_target` is that root, not the button; `target()`
            // may land on an inner span, so `closest` walks up to the card.
            let grab_offset = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                .and_then(|element| element.closest(".card").ok().flatten())
                .map(|card| {
                    let rect = card.get_bounding_client_rect();
                    PointerPoint {
                        x: at.x - rect.x(),
                        y: at.y - rect.y(),
                    }
                })
                .unwrap_or(PointerPoint { x: 0.0, y: 0.0 });
            Msg::PointerDown(origin, event.pointer_id(), at, grab_offset)
        });
        let move_ = link.callback(|event: PointerEvent| {
            Msg::PointerMove(
                event.pointer_id(),
                event.client_x() as f64,
                event.client_y() as f64,
            )
        });
        let up = link.callback(|event: PointerEvent| {
            release_pointer(&event);
            Msg::PointerUp(
                event.pointer_id(),
                event.client_x() as f64,
                event.client_y() as f64,
            )
        });
        let cancel = link.callback(|event: PointerEvent| {
            release_pointer(&event);
            Msg::PointerCancel(event.pointer_id())
        });
        PointerCallbacks {
            down,
            move_,
            up,
            cancel,
        }
    }

    /// Reads whatever tableau column or foundation lies under a client-space
    /// point via `elementFromPoint`. DOM-dependent, so it stays out of
    /// `cargo test`; proven in the browser instead.
    fn hit_test_drop_target(x: f64, y: f64) -> Option<DropTarget> {
        let document = web_sys::window()?.document()?;
        let element = document.element_from_point(x as f32, y as f32)?;
        let target = element
            .closest("[data-drop-tableau], [data-drop-foundation]")
            .ok()??;
        if let Some(value) = target.get_attribute("data-drop-tableau") {
            return value.parse().ok().map(DropTarget::Tableau);
        }
        let value = target.get_attribute("data-drop-foundation")?;
        value.parse().ok().map(DropTarget::Foundation)
    }

    /// Resolves a drag's drop against the hit-tested target, moving through
    /// the same `GameState` calls the click path uses and wording the result
    /// the same way. `target` is `None` when the pointer released outside
    /// any drop zone, which is always illegal.
    fn resolve_drop(&mut self, target: Option<DropTarget>) -> DropOutcome {
        match target {
            Some(DropTarget::Tableau(pile)) => {
                if self.game.move_selected_to_tableau(pile) {
                    DropOutcome {
                        moved: true,
                        status: Self::tableau_move_status(pile),
                    }
                } else {
                    DropOutcome {
                        moved: false,
                        status: self.tableau_rejection_status(pile),
                    }
                }
            }
            Some(DropTarget::Foundation(pile)) => {
                if self.game.move_selected_to_foundation(pile) {
                    DropOutcome {
                        moved: true,
                        status: Self::foundation_move_status(pile),
                    }
                } else {
                    DropOutcome {
                        moved: false,
                        status: ILLEGAL_FOUNDATION_MOVE.to_string(),
                    }
                }
            }
            None => DropOutcome {
                moved: false,
                status: ILLEGAL_TABLEAU_MOVE.to_string(),
            },
        }
    }

    /// Resolves a tap on tableau `pile`: extends the selection onto it, or
    /// without a selection, selects its top face-up card. Words a rejected
    /// move the same way `resolve_drop` does.
    fn click_tableau_pile(&mut self, ctx: Option<&Context<Self>>, pile: usize) {
        if self.game.selected.is_some() {
            if self.apply_move_with_flight(ctx, |game| game.move_selected_to_tableau(pile)) {
                self.status = Self::tableau_move_status(pile);
            } else {
                self.status = self.tableau_rejection_status(pile);
            }
        } else if let Some(top_index) = self.game.tableau[pile].len().checked_sub(1)
            && self.game.tableau[pile][top_index].face_up
            && self.game.select_tableau(pile, top_index)
            && let Some(card) = self.game.selected_card()
        {
            self.status = format!("Selected top card {}.", Self::describe_card(card));
        }
    }

    /// The cards a drag from `origin` carries: the exact selection payload,
    /// read straight from `GameState` rather than tracked separately.
    fn dragged_cards(&self, origin: Selection) -> Vec<Card> {
        match origin {
            Selection::Waste => self.game.waste.last().copied().into_iter().collect(),
            Selection::Foundation { pile } => self
                .game
                .foundations
                .get(pile)
                .and_then(|cards| cards.last())
                .copied()
                .into_iter()
                .collect(),
            Selection::Tableau { pile, index } => self
                .game
                .tableau
                .get(pile)
                .and_then(|cards| cards.get(index..))
                .map(|run| run.iter().map(|card| card.card).collect())
                .unwrap_or_default(),
        }
    }

    fn is_flying(&self, card: Card) -> bool {
        self.flights.iter().any(|flight| flight.card == card)
    }

    /// The cards currently lifted off their pile by an active drag past the
    /// tap threshold — hidden at their origin so the overlay is their only
    /// visible copy. Empty for a press that never crossed the threshold, so
    /// a tap never blinks.
    fn lifted_cards(&self) -> Vec<Card> {
        self.drag
            .as_ref()
            .filter(|tracker| tracker.phase == DragPhase::Dragging)
            .map(|tracker| self.dragged_cards(tracker.origin))
            .unwrap_or_default()
    }

    /// Whether a face-up card should paint hidden: away on a flight, or
    /// lifted off its pile mid-drag.
    fn is_away(&self, card: Card, lifted: &[Card]) -> bool {
        self.is_flying(card) || lifted.contains(&card)
    }

    fn rect_for_stock_slot() -> Option<Rect> {
        element_rect("[data-pile-slot='stock']")
    }

    /// Every currently visible face-up card's departure rect, read from the
    /// DOM before `mutate` changes `GameState`: a card already mid-flight
    /// departs from its live flight-layer position, not its stale slot.
    fn capture_departure_rects(&self, before: &motion::Snapshot) -> Vec<(Card, Rect)> {
        before
            .iter()
            .filter_map(|(card, _)| {
                let key = motion::card_key(*card);
                let board = element_rect(&format!("[data-card-id='{key}']"));
                let in_flight = self
                    .is_flying(*card)
                    .then(|| element_rect(&format!("[data-flight-card='{key}']")))
                    .flatten();
                match (board, in_flight) {
                    (Some(board), in_flight) => {
                        Some((*card, motion::departure_rect(board, in_flight)))
                    }
                    (None, Some(in_flight)) => Some((*card, in_flight)),
                    (None, None) => None,
                }
            })
            .collect()
    }

    /// A dragged card's departure rect at release: wherever the overlay
    /// last drew it, read from the DOM before the overlay is torn down.
    fn capture_overlay_departure_rects(&self, dragged: &[Card]) -> Vec<(Card, Rect)> {
        dragged
            .iter()
            .filter_map(|card| {
                let selector = format!("[data-overlay-card='{}']", motion::card_key(*card));
                element_rect(&selector).map(|rect| (*card, rect))
            })
            .collect()
    }

    /// Applies one GameState-mutating move and, on success, plans and
    /// launches flights for every card its snapshot diff calls moved.
    /// Wraps `move_selected_to_tableau`/`_foundation` and
    /// `auto_promote_lowest` identically, so every click, double-click and
    /// auto-promotion path gains motion from one call.
    /// `ctx` is `None` only from a hermetic unit test, where no real Yew
    /// scope exists to schedule a flight's expiry timer against; the move
    /// itself still applies, just without ever entering `self.flights`.
    fn apply_move_with_flight<F>(&mut self, ctx: Option<&Context<Self>>, mutate: F) -> bool
    where
        F: FnOnce(&mut GameState) -> bool,
    {
        // No Yew scope: a hermetic unit test. Skip every DOM touch (even
        // `prefers_reduced_motion`'s `window()`) and just apply the move,
        // matching plain `move_selected_to_*` from before flights existed.
        let Some(ctx) = ctx else {
            return mutate(&mut self.game);
        };
        let reduced_motion = prefers_reduced_motion();
        let before = motion::snapshot(&self.game);
        let departures = if reduced_motion {
            Vec::new()
        } else {
            self.capture_departure_rects(&before)
        };
        let moved = mutate(&mut self.game);
        if moved {
            let after = motion::snapshot(&self.game);
            let launched_at = js_sys::Date::now();
            let flights = motion::plan_flights_for_move(
                &before,
                &after,
                &departures,
                FlightKind::Travel,
                reduced_motion,
                launched_at,
            );
            self.launch_flights(ctx, flights);
        }
        moved
    }

    /// Appends each flight and arms its deadline: a flight always lands,
    /// even when its `transitionend` never fires (an element removed, a
    /// tab hidden, reduced motion switched on mid-flight).
    fn launch_flights(&mut self, ctx: &Context<Self>, flights: Vec<Flight>) {
        for flight in flights {
            let card = flight.card;
            let launched_at = flight.launched_at;
            // A card moved again mid-flight supersedes its own earlier
            // flight rather than flying twice at once; the old timer still
            // fires later, but by then this card's launched_at has moved
            // on, so `ExpireFlight` finds nothing to remove.
            self.flights.retain(|existing| existing.card != card);
            let link = ctx.link().clone();
            Timeout::new(flight.kind.deadline_ms() as u32, move || {
                link.send_message(Msg::ExpireFlight(card, launched_at));
            })
            .forget();
            self.flights.push(flight);
        }
    }

    /// Measures the destination of every flight still awaiting one. Called
    /// from `rendered()`, after the move's own render has already painted
    /// the card hidden at its new home — that board copy's rect, though
    /// invisible, is now the correct landing point.
    fn measure_flight_destinations(&self, ctx: &Context<Self>) {
        let measured: Vec<(Card, f64, Rect)> = self
            .flights
            .iter()
            .filter(|flight| flight.to.is_none())
            .filter_map(|flight| {
                let selector = format!("[data-card-id='{}']", motion::card_key(flight.card));
                element_rect(&selector).map(|rect| (flight.card, flight.launched_at, rect))
            })
            .collect();
        if !measured.is_empty() {
            ctx.link()
                .send_message(Msg::FlightDestinationsMeasured(measured));
        }
    }

    /// The red/black and court-card classes every face rendering shares.
    fn card_palette_classes(card: Card) -> Classes {
        let mut classes = classes!(if card.is_red() { "red" } else { "black" });
        if matches!(card.rank, 1 | 11 | 12 | 13) {
            classes.push("court");
        }
        classes
    }

    /// The face markup a board card, the drag overlay and the flight layer
    /// all share, so a card's face never changes at pick-up, release or
    /// landing.
    fn card_face_content(card: Card) -> Html {
        let center_art = if matches!(card.rank, 11..=13) {
            "art-dionysus"
        } else if card.rank == 1 {
            "art-temple"
        } else {
            "art-laurel"
        };
        html! {
            <>
                <span class="corner top">
                    <span class="rank">{ card.rank_label() }</span>
                    <span class="suit">{ card.suit.symbol() }</span>
                </span>
                <span class="center">
                    <span class={classes!("center-art", center_art)} aria-hidden="true"></span>
                    <span class="glyph">{ card.suit.symbol() }</span>
                    <span class="motif">{ card.motif() }</span>
                </span>
                <span class="corner bottom">
                    <span class="rank">{ card.rank_label() }</span>
                    <span class="suit">{ card.suit.symbol() }</span>
                </span>
            </>
        }
    }

    /// A pile whose visible top is away shows what lies beneath it instead
    /// of going blank: waste and foundation render only their top card, so
    /// hiding it without an underlay would leave the slot empty for the
    /// flight's whole length.
    fn view_pile_underlay(pile: &[Card], flights: &[Flight], empty_label: (&str, &str)) -> Html {
        let beneath = &pile[..pile.len().saturating_sub(1)];
        match motion::underlay_card(beneath, flights) {
            Some(card) => {
                let mut classes = classes!("card", "face", "pile-underlay");
                classes.extend(Self::card_palette_classes(card));
                html! {
                    <div class={classes} aria-hidden="true">
                        { Self::card_face_content(card) }
                    </div>
                }
            }
            None => html! {
                <div class="pile-empty pile-underlay" aria-hidden="true">
                    <span>{ empty_label.0 }</span>
                    <span class="tiny">{ empty_label.1 }</span>
                </div>
            },
        }
    }

    /// Every card in flight, each its own fixed-position layer so it can
    /// travel to its own measured destination independently — a run flies
    /// as a run, but each card keeps its own fan slot.
    fn view_flight_layer(&self) -> Html {
        self.flights
            .iter()
            .map(|flight| {
                let rect = flight.to.unwrap_or(flight.from);
                let mut classes = classes!("card", "face", "flight-card");
                classes.extend(Self::card_palette_classes(flight.card));
                if flight.kind == FlightKind::SettleBack {
                    classes.push("settle-back");
                }
                let style = format!("transform: translate({}px, {}px);", rect.x, rect.y);
                let card_id = motion::card_key(flight.card);
                html! {
                    <div
                        key={format!("flight-{card_id}")}
                        class={classes}
                        style={style}
                        aria-hidden="true"
                        data-flight-card={card_id}
                    >
                        { Self::card_face_content(flight.card) }
                    </div>
                }
            })
            .collect::<Html>()
    }

    /// The dragged run's ghost, a single layer outside `.tableau-scroll` that
    /// follows the pointer by `transform` alone so the clipped, scrollable
    /// column never has to reflow mid-drag.
    fn view_drag_overlay(&self) -> Html {
        let Some(tracker) = self
            .drag
            .as_ref()
            .filter(|tracker| tracker.phase == DragPhase::Dragging)
        else {
            return Html::default();
        };

        let cards = self.dragged_cards(tracker.origin);
        // Keeps the grab point under the pointer rather than centering the
        // card on it — hiding the origin (below) would expose that jump.
        let overlay_style = format!(
            "transform: translate({}px, {}px);",
            tracker.current.x - tracker.grab_offset.x,
            tracker.current.y - tracker.grab_offset.y,
        );

        html! {
            <div class="drag-overlay" style={overlay_style} aria-hidden="true">
                { for cards.iter().enumerate().map(|(index, card)| {
                    let mut card_classes = classes!("card", "face", "drag-overlay-card");
                    card_classes.extend(Self::card_palette_classes(*card));
                    html! {
                        <div
                            key={motion::card_key(*card)}
                            class={card_classes}
                            style={format!("--fan-index: {index};")}
                            data-overlay-card={motion::card_key(*card)}
                        >
                            { Self::card_face_content(*card) }
                        </div>
                    }
                }) }
            </div>
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn view_face_card(
        &self,
        card: Card,
        selected: bool,
        zeus_revealed: bool,
        away: bool,
        on_click: yew::Callback<MouseEvent>,
        on_double_click: yew::Callback<MouseEvent>,
        pointer: PointerCallbacks,
        drop_foundation: Option<usize>,
        drop_target: bool,
    ) -> Html {
        let mut card_classes = classes!("card", "face");
        card_classes.extend(Self::card_palette_classes(card));
        if selected {
            card_classes.push("selected");
        }
        if zeus_revealed {
            card_classes.push("zeus-revealed");
        }
        if drop_target {
            card_classes.push("drop-target");
        }
        if away {
            // Opacity, not visibility/display: a card mid-flight or lifted
            // by a drag must stay hit-testable and clickable (§1).
            card_classes.push("away");
        }
        let card_id = motion::card_key(card);

        html! {
            <button
                type="button"
                key={card_id.clone()}
                class={card_classes}
                onclick={on_click}
                ondblclick={on_double_click}
                onpointerdown={pointer.down}
                onpointermove={pointer.move_}
                onpointerup={pointer.up}
                onpointercancel={pointer.cancel}
                aria-label={format!("{} of {}", card.rank_label(), card.suit.latin_name())}
                disabled={self.interactions_locked()}
                data-drop-foundation={drop_foundation.map(|pile| pile.to_string())}
                data-card-id={card_id}
            >
                { Self::card_face_content(card) }
            </button>
        }
    }

    fn view_back_card(
        &self,
        card: Card,
        selected: bool,
        label: &'static str,
        on_click: yew::Callback<MouseEvent>,
        pile_slot: Option<&'static str>,
    ) -> Html {
        let mut card_classes = classes!("card", "back");
        if selected {
            card_classes.push("selected");
        }

        html! {
            <button
                type="button"
                key={motion::card_key(card)}
                class={card_classes}
                onclick={on_click}
                aria-label={label}
                disabled={self.interactions_locked()}
                data-pile-slot={pile_slot}
            >
                <span class="back-medallion" aria-hidden="true"></span>
            </button>
        }
    }

    fn view_foundation_slot(&self, ctx: &Context<Self>, pile: usize, lifted: &[Card]) -> Html {
        let on_click = ctx.link().callback(move |_| Msg::ClickFoundation(pile));
        let selected = self.game.is_selected(Selection::Foundation { pile });
        let drop_target = self.hover_target == Some(DropTarget::Foundation(pile));
        let foundation = &self.game.foundations[pile];
        let top_away = foundation
            .last()
            .is_some_and(|card| self.is_away(*card, lifted));
        let underlay = top_away
            .then(|| Self::view_pile_underlay(foundation, &self.flights, ("TEMPLE", "ACE UP")));

        if let Some(card) = foundation.last().copied() {
            let on_double_click = ctx.link().callback(|_| Msg::Noop);
            let pointer = Self::pointer_callbacks(ctx, Selection::Foundation { pile });
            let face = self.view_face_card(
                card,
                selected,
                false,
                top_away,
                on_click,
                on_double_click,
                pointer,
                Some(pile),
                drop_target,
            );
            html! {
                <div class="card-frame">
                    { for underlay }
                    { face }
                </div>
            }
        } else {
            html! {
                <button
                    type="button"
                    class={classes!(
                        "pile-empty",
                        selected.then_some("selected"),
                        drop_target.then_some("drop-target"),
                    )}
                    onclick={on_click}
                    aria-label={format!("Foundation {}", pile + 1)}
                    disabled={self.interactions_locked()}
                    data-drop-foundation={pile.to_string()}
                >
                    <span>{ "TEMPLE" }</span>
                    <span class="tiny">{ "ACE UP" }</span>
                </button>
            }
        }
    }
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(_: &Context<Self>) -> Self {
        let mut game = GameState::new_shuffled();
        game.temple_gold = load_temple_gold();
        Self {
            game,
            status: "Draw from stock and build the four temples from Ace to King.".to_string(),
            end_state: None,
            help_expanded: false,
            victory_gold_award: 0,
            victory_rain_dismissed: false,
            all_to_temple_running: false,
            all_to_temple_timeout: None,
            key_listener: None,
            drag: None,
            just_dragged: false,
            hover_target: None,
            flights: Vec::new(),
        }
    }

    fn rendered(&mut self, ctx: &Context<Self>, first_render: bool) {
        // A flight always lands: this sweep is a second guarantee beside
        // each flight's own `ExpireFlight` timeout, catching a flight that
        // for any reason outlived its deadline before that timer fired.
        let now = js_sys::Date::now();
        self.flights.retain(|flight| !flight.has_expired(now));

        // Every render, not just the first: a flight's destination can only
        // be measured once its (hidden) board copy has actually painted.
        self.measure_flight_destinations(ctx);

        if !first_render || self.key_listener.is_some() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let link = ctx.link().clone();
        let listener = EventListener::new(&document, "keydown", move |event| {
            let Some(event) = event.dyn_ref::<DomKeyboardEvent>() else {
                return;
            };
            if event.repeat() || event.ctrl_key() || event.meta_key() || event.alt_key() {
                return;
            }

            let key = event.key();
            let msg = match key.as_str() {
                "d" | "D" => Some(Msg::DrawStock),
                "a" | "A" => Some(Msg::AllToTemple),
                " " => Some(Msg::AutoFoundation),
                "Enter" => {
                    // Skip when a button already has focus so Enter still
                    // activates the focused control via the browser instead
                    // of double-firing DrawStock.
                    let button_focused = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.active_element())
                        .is_some_and(|e| e.tag_name().eq_ignore_ascii_case("BUTTON"));
                    if button_focused {
                        None
                    } else {
                        Some(Msg::DrawStock)
                    }
                }
                _ => None,
            };

            if let Some(msg) = msg {
                event.prevent_default();
                link.send_message(msg);
            }
        });
        self.key_listener = Some(listener);
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        // Flight bookkeeping passes the end-state gate: the winning move's
        // own flight must still land while the victory screen shows.
        if self.end_state.is_some()
            && !matches!(
                msg,
                Msg::Noop
                    | Msg::NewGame
                    | Msg::ToggleHelp
                    | Msg::DismissVictoryRain
                    | Msg::FlightDestinationsMeasured(_)
                    | Msg::ExpireFlight(..)
            )
        {
            self.status = match self.end_state {
                Some(EndState::ZeusThunder) => "Zeus' Thunder is heard".to_string(),
                Some(EndState::OutOfGold) => {
                    "Temple Gold has run out. You lose. Final vision reveals all cards.".to_string()
                }
                Some(EndState::Victory) => {
                    let victory_gold_award = self.victory_gold_award;
                    format!("Dionysus honors you with {victory_gold_award} gold.")
                }
                Some(EndState::Stalemate) => {
                    "Zeus demands obeisance! Only surrender remains.".to_string()
                }
                None => self.status.clone(),
            };
            return true;
        }

        match msg {
            Msg::Noop => return false,
            Msg::NewGame => {
                let carry_gold = if matches!(self.end_state, Some(EndState::Victory)) {
                    self.game.temple_gold
                } else {
                    0
                };
                let draw_count = self.game.draw_count;
                self.game = GameState::new_shuffled_with_draw_count(draw_count);
                self.game.temple_gold = carry_gold;
                self.stop_all_to_temple();
                self.end_state = None;
                self.victory_gold_award = 0;
                self.victory_rain_dismissed = false;
                self.status = "You gave up. A fresh deck has been dealt.".to_string();
                self.flights.clear();
            }
            Msg::DrawStock => {
                let had_stock = !self.game.stock.is_empty();
                let had_waste = !self.game.waste.is_empty();
                let waste_before = self.game.waste.len();
                let gold_before = self.game.temple_gold;
                let reduced_motion = prefers_reduced_motion();
                // The new waste top departs from the stock slot, already
                // face up: measure before the draw pops the stock's card.
                let stock_rect = (had_stock && !reduced_motion)
                    .then(Self::rect_for_stock_slot)
                    .flatten();
                self.game.draw_or_recycle();
                self.status = if had_stock {
                    let drawn = self.game.waste.len().saturating_sub(waste_before);
                    let suffix = if drawn == 1 { "" } else { "s" };
                    format!("Drew {drawn} card{suffix} to the waste pile.")
                } else if had_waste {
                    // A recycle cancels any flight in the air; nothing new
                    // flies here.
                    self.flights.clear();
                    let collected = gold_before.saturating_sub(self.game.temple_gold);
                    if collected > 0 {
                        format!(
                            "Recycled waste back into stock. Temple collected {collected} gold."
                        )
                    } else {
                        "Recycled waste back into stock.".to_string()
                    }
                } else {
                    "No cards available to draw.".to_string()
                };

                if let Some(stock_rect) = stock_rect {
                    let drawn = self.game.waste.len().saturating_sub(waste_before);
                    let new_top = self.game.waste.last().copied();
                    let launched_at = js_sys::Date::now();
                    if let Some(flight) =
                        motion::plan_draw_flight(new_top, stock_rect, reduced_motion, launched_at)
                    {
                        // Draw-3's lower two cards arrive silently: only the
                        // true top gets a flight, so the underlay must skip
                        // them too until it lands.
                        let covers = if drawn > 1 {
                            let waste_len = self.game.waste.len();
                            self.game.waste[waste_len - drawn..waste_len - 1].to_vec()
                        } else {
                            Vec::new()
                        };
                        self.launch_flights(ctx, vec![flight.with_covers(covers)]);
                    }
                }

                if !had_stock && had_waste && self.game.temple_gold == 0 {
                    self.game.zeus_vision();
                    self.stop_all_to_temple();
                    self.end_state = Some(EndState::OutOfGold);
                    self.status =
                        "Temple Gold has run out. You lose. Final vision reveals all cards."
                            .to_string();
                }
            }
            Msg::ClickWaste => {
                if self.just_dragged {
                    return false;
                }
                if self.game.select_waste() {
                    if let Some(card) = self.game.selected_card() {
                        self.status = format!("Selected waste card {}.", Self::describe_card(card));
                    } else {
                        self.status = "Selection cleared.".to_string();
                    }
                } else {
                    self.status = "Waste pile is empty.".to_string();
                }
            }
            Msg::DoubleClickWaste => {
                if self.just_dragged {
                    return false;
                }
                if self.game.waste.is_empty() {
                    self.status = "Waste pile is empty.".to_string();
                } else {
                    let _ = self.game.select_waste();
                    if self.apply_move_with_flight(Some(ctx), |game| {
                        game.move_selected_to_any_foundation()
                    }) {
                        self.status = "Moved waste card to a foundation.".to_string();
                    } else {
                        self.game.clear_selection();
                        self.status = "Waste card cannot move to any foundation yet.".to_string();
                    }
                }
            }
            Msg::ClickFoundation(pile) => {
                if self.just_dragged {
                    return false;
                }
                if self.game.selected.is_some() {
                    if self.apply_move_with_flight(Some(ctx), |game| {
                        game.move_selected_to_foundation(pile)
                    }) {
                        self.status = Self::foundation_move_status(pile);
                    } else if self.game.select_foundation(pile) {
                        if let Some(card) = self.game.selected_card() {
                            self.status = format!(
                                "Selected foundation card {} to move back.",
                                Self::describe_card(card)
                            );
                        } else {
                            self.status = "Selection cleared.".to_string();
                        }
                    } else {
                        self.status = ILLEGAL_FOUNDATION_MOVE.to_string();
                    }
                } else if self.game.select_foundation(pile) {
                    if let Some(card) = self.game.selected_card() {
                        self.status = format!(
                            "Selected foundation card {} to move back.",
                            Self::describe_card(card)
                        );
                    }
                } else {
                    self.status = format!("Foundation {} is empty.", pile + 1);
                }
            }
            Msg::ClickTableauCard(pile, index) => {
                if self.just_dragged {
                    return false;
                }
                if self.game.selected.is_some() {
                    if self.apply_move_with_flight(Some(ctx), |game| {
                        game.move_selected_to_tableau(pile)
                    }) {
                        self.status = Self::tableau_move_status(pile);
                    } else if self.game.select_tableau(pile, index) {
                        if let Some(card) = self.game.selected_card() {
                            self.status = format!(
                                "Selected tableau run starting at {}.",
                                Self::describe_card(card)
                            );
                        }
                    } else {
                        self.status = ILLEGAL_TABLEAU_MOVE.to_string();
                    }
                } else if self.game.select_tableau(pile, index) {
                    if let Some(card) = self.game.selected_card() {
                        self.status = format!(
                            "Selected tableau run starting at {}.",
                            Self::describe_card(card)
                        );
                    }
                } else {
                    self.status = "That card is blocked by game rules.".to_string();
                }
            }
            Msg::DoubleClickTableauCard(pile, index) => {
                // Guarded like Click*: a drag's mouseup can synthesize a
                // trailing click AND dblclick on the source card (most
                // visibly in the tap-then-drag sequence), and an unguarded
                // dblclick here would reselect/promote against a pile the
                // drag just changed.
                if self.just_dragged {
                    return false;
                }
                if self.game.select_tableau(pile, index) {
                    if self.apply_move_with_flight(Some(ctx), |game| {
                        game.move_selected_to_any_foundation()
                    }) {
                        self.status = "Moved top tableau card to a foundation.".to_string();
                    } else {
                        self.game.clear_selection();
                        self.status = "No legal foundation move for that card.".to_string();
                    }
                } else {
                    self.status = "Only exposed cards can jump to foundations.".to_string();
                }
            }
            Msg::ClickTableauPile(pile) => {
                // Guarded like the other three Click* handlers (§1): inert
                // today only because cards stop_propagation() and pointer
                // capture retarget the trailing click away from the pile —
                // accident, not a rule this handler can rely on.
                if self.just_dragged {
                    return false;
                }
                self.click_tableau_pile(Some(ctx), pile);
            }
            Msg::AutoFoundation => {
                if self.apply_move_with_flight(Some(ctx), |game| game.auto_promote_lowest()) {
                    self.status = "Moved one card to a foundation.".to_string();
                } else {
                    self.status = "No automatic foundation move available.".to_string();
                }
            }
            Msg::AllToTemple => {
                if self.all_to_temple_running {
                    return false;
                }

                self.game.clear_selection();
                if self.apply_move_with_flight(Some(ctx), |game| game.auto_promote_lowest()) {
                    if self.game.won {
                        self.trigger_victory();
                    } else {
                        self.all_to_temple_running = true;
                        self.status = "All available cards are marching to the temple.".to_string();
                        self.schedule_all_to_temple_step(ctx);
                    }
                } else {
                    self.status = "No automatic temple moves available.".to_string();
                }
            }
            Msg::AllToTempleStep => {
                self.all_to_temple_timeout = None;
                if !self.all_to_temple_running {
                    return false;
                }

                if self.apply_move_with_flight(Some(ctx), |game| game.auto_promote_lowest()) {
                    if self.game.won {
                        self.trigger_victory();
                    } else {
                        self.schedule_all_to_temple_step(ctx);
                    }
                } else {
                    self.stop_all_to_temple();
                    self.status = "All possible cards have been moved to temple.".to_string();
                }
            }
            Msg::ZeusVision => {
                self.stop_all_to_temple();
                self.game.zeus_vision();
                self.game.temple_gold = 0;
                self.end_state = Some(EndState::ZeusThunder);
                self.status = "Zeus' Thunder is heard".to_string();
                // Nothing flies on Zeus' Vision; the reveal re-fans the
                // whole board, so any flight in the air would land wrong.
                self.flights.clear();
            }
            Msg::SwitchDrawMode => {
                let next = if self.game.draw_count == EASY_DRAW_COUNT {
                    HARD_DRAW_COUNT
                } else {
                    EASY_DRAW_COUNT
                };
                self.game.set_draw_count(next);
                self.status = if next == EASY_DRAW_COUNT {
                    "Easy mode: draw 1 card from stock.".to_string()
                } else {
                    "Hard mode: draw 3 cards from stock.".to_string()
                };
            }
            Msg::ToggleHelp => {
                self.help_expanded = !self.help_expanded;
                self.status = if self.help_expanded {
                    "Help expanded.".to_string()
                } else {
                    "Help minimized.".to_string()
                };
            }
            Msg::DismissVictoryRain => {
                if matches!(self.end_state, Some(EndState::Victory)) && !self.victory_rain_dismissed
                {
                    self.victory_rain_dismissed = true;
                } else {
                    return false;
                }
            }
            Msg::PointerDown(origin, pointer_id, at, grab_offset) => {
                // Defensive reset: `schedule_just_dragged_clear` already
                // guarantees the flag from a prior drag is gone before any
                // unrelated later gesture, but a fresh press should never
                // see it lingering even in that already-cleared state.
                self.just_dragged = false;
                if self.interactions_locked() || self.drag.is_some() {
                    return false;
                }
                let selection_before = self.game.selected;
                self.drag = Some(DragTracker::new(
                    pointer_id,
                    origin,
                    selection_before,
                    at,
                    grab_offset,
                ));
                return false;
            }
            Msg::PointerMove(pointer_id, x, y) => {
                let Some(tracker) = self.drag.as_ref() else {
                    return false;
                };
                if tracker.pointer_id != pointer_id {
                    return false;
                }
                let was_dragging = tracker.phase == DragPhase::Dragging;
                let advanced = tracker.moved(PointerPoint { x, y });
                let origin = advanced.origin;
                let just_started = !was_dragging && advanced.phase == DragPhase::Dragging;
                self.drag = Some(advanced);

                // The select_* calls toggle an already-active selection off,
                // so a drag that starts on an already-selected card must
                // skip reselecting it rather than deselect it mid-gesture.
                if just_started && !self.game.is_selected(origin) {
                    match origin {
                        Selection::Waste => {
                            self.game.select_waste();
                        }
                        Selection::Foundation { pile } => {
                            self.game.select_foundation(pile);
                        }
                        Selection::Tableau { pile, index } => {
                            self.game.select_tableau(pile, index);
                        }
                    }
                }

                self.hover_target = self
                    .drag
                    .as_ref()
                    .filter(|tracker| tracker.phase == DragPhase::Dragging)
                    .and_then(|_| Self::hit_test_drop_target(x, y));
                // Selecting or repositioning the overlay never wins or
                // stalls the game; skip the shared recheck below.
                return true;
            }
            Msg::PointerUp(pointer_id, x, y) => {
                let Some(tracker) = self.drag.take() else {
                    return false;
                };
                if tracker.pointer_id != pointer_id {
                    self.drag = Some(tracker);
                    return false;
                }
                self.hover_target = None;
                if tracker.phase == DragPhase::Pressed {
                    // A tap: leave game state untouched for the browser's
                    // own click/dblclick to drive as it does today.
                    return false;
                }

                // The overlay is still on screen at this point (this
                // render hasn't patched the DOM yet), so its cards' rects
                // are exactly where it last drew them.
                let reduced_motion = prefers_reduced_motion();
                let dragged = self.dragged_cards(tracker.origin);
                let departures = if reduced_motion {
                    Vec::new()
                } else {
                    self.capture_overlay_departure_rects(&dragged)
                };

                let target = Self::hit_test_drop_target(x, y);
                let outcome = self.resolve_drop(target);
                self.just_dragged = true;
                Self::schedule_just_dragged_clear(ctx);
                if !outcome.moved {
                    self.game.selected = tracker.selection_before;
                }
                if !reduced_motion {
                    let kind = if outcome.moved {
                        FlightKind::Travel
                    } else {
                        FlightKind::SettleBack
                    };
                    let launched_at = js_sys::Date::now();
                    let flights: Vec<Flight> = dragged
                        .iter()
                        .filter_map(|card| {
                            departures
                                .iter()
                                .find(|(c, _)| c == card)
                                .map(|(_, rect)| Flight::new(*card, *rect, kind, launched_at))
                        })
                        .collect();
                    self.launch_flights(ctx, flights);
                }
                self.status = outcome.status;
            }
            Msg::PointerCancel(pointer_id) => {
                let Some(tracker) = self.drag.take() else {
                    return false;
                };
                if tracker.pointer_id != pointer_id {
                    self.drag = Some(tracker);
                    return false;
                }
                let reduced_motion = prefers_reduced_motion();
                let dragged = self.dragged_cards(tracker.origin);
                let departures = if reduced_motion {
                    Vec::new()
                } else {
                    self.capture_overlay_departure_rects(&dragged)
                };
                self.game.selected = tracker.selection_before;
                self.hover_target = None;
                if !reduced_motion {
                    let launched_at = js_sys::Date::now();
                    let flights: Vec<Flight> = dragged
                        .iter()
                        .filter_map(|card| {
                            departures.iter().find(|(c, _)| c == card).map(|(_, rect)| {
                                Flight::new(*card, *rect, FlightKind::SettleBack, launched_at)
                            })
                        })
                        .collect();
                    self.launch_flights(ctx, flights);
                }
            }
            Msg::ClearJustDragged => {
                self.just_dragged = false;
                return false;
            }
            Msg::FlightDestinationsMeasured(measurements) => {
                for (card, launched_at, rect) in measurements {
                    if let Some(flight) = self.flights.iter_mut().find(|flight| {
                        flight.card == card
                            && flight.launched_at == launched_at
                            && flight.to.is_none()
                    }) {
                        flight.to = Some(rect);
                    }
                }
                return true;
            }
            Msg::ExpireFlight(card, launched_at) => {
                self.flights
                    .retain(|flight| !(flight.card == card && flight.launched_at == launched_at));
                return true;
            }
        }

        if self.game.won && self.end_state.is_none() {
            self.trigger_victory();
        }
        if self.end_state.is_none() && !self.game.has_any_legal_move() {
            self.stop_all_to_temple();
            self.game.zeus_vision();
            self.game.temple_gold = 0;
            self.end_state = Some(EndState::Stalemate);
            self.status = "Zeus demands obeisance! Only surrender remains.".to_string();
        }

        persist_temple_gold(self.game.temple_gold);
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let draw_stock = ctx.link().callback(|_| Msg::DrawStock);
        let new_game = ctx.link().callback(|_| Msg::NewGame);
        let auto_foundation = ctx.link().callback(|_| Msg::AutoFoundation);
        let all_to_temple = ctx.link().callback(|_| Msg::AllToTemple);
        let zeus_vision = ctx.link().callback(|_| Msg::ZeusVision);
        let switch_draw_mode = ctx.link().callback(|_| Msg::SwitchDrawMode);
        let toggle_help = ctx.link().callback(|_| Msg::ToggleHelp);
        let click_waste = ctx.link().callback(|_| Msg::ClickWaste);
        let double_click_waste = ctx.link().callback(|_| Msg::DoubleClickWaste);
        let locked = self.end_state.is_some();
        let actions_busy = self.all_to_temple_running;
        let easy_mode_active = self.game.draw_count == EASY_DRAW_COUNT;
        let mode_label = if self.game.draw_count == HARD_DRAW_COUNT {
            "Hard (Draw 3)"
        } else {
            "Easy (Draw 1)"
        };
        let reset_label = if matches!(self.end_state, Some(EndState::Victory)) {
            "Play Again"
        } else {
            "Give Up!"
        };
        let help_button_label = if self.help_expanded {
            "Minimize Help"
        } else {
            "Expand Help"
        };

        // Which cards a live drag has lifted off their pile — computed once
        // and threaded through every pile view that needs to hide one.
        let lifted = self.lifted_cards();

        let stock_view = if self.game.stock.is_empty() {
            let label = if self.game.waste.is_empty() {
                "Stock"
            } else {
                "Recycle waste"
            };
            html! {
                <button type="button" class="pile-empty stock-empty" onclick={draw_stock.clone()} aria-label={label} disabled={locked} data-pile-slot="stock">
                    <span>{ "REDEAL" }</span>
                    <span class="tiny">{ "STOCK" }</span>
                </button>
            }
        } else {
            // Non-empty per the branch above: the top card's identity keys
            // this button and marks the slot flights depart from.
            let top = self.game.stock.last().copied().expect("stock non-empty");
            self.view_back_card(
                top,
                false,
                "Draw from stock",
                draw_stock.clone(),
                Some("stock"),
            )
        };

        let waste_view = if let Some(card) = self.game.waste.last().copied() {
            let selected = self.game.is_selected(Selection::Waste);
            let away = self.is_away(card, &lifted);
            let underlay = away.then(|| {
                Self::view_pile_underlay(&self.game.waste, &self.flights, ("WASTE", "DRAW"))
            });
            let pointer = Self::pointer_callbacks(ctx, Selection::Waste);
            let face = self.view_face_card(
                card,
                selected,
                false,
                away,
                click_waste,
                double_click_waste,
                pointer,
                None,
                false,
            );
            html! {
                <div class="card-frame">
                    { for underlay }
                    { face }
                </div>
            }
        } else {
            html! {
                <button
                    type="button"
                    class="pile-empty"
                    onclick={click_waste}
                    aria-label="Waste pile"
                    disabled={locked}
                >
                    <span>{ "WASTE" }</span>
                    <span class="tiny">{ "DRAW" }</span>
                </button>
            }
        };

        let foundation_slots = (0..4)
            .map(|pile| {
                html! {
                    <div class="pile-slot">
                        <div class="pile-label">{ format!("Temple {}", pile + 1) }</div>
                        { self.view_foundation_slot(ctx, pile, &lifted) }
                    </div>
                }
            })
            .collect::<Html>();

        let tableau_columns = self
            .game
            .tableau
            .iter()
            .enumerate()
            .map(|(pile_index, pile)| {
                let pile_click = ctx
                    .link()
                    .callback(move |_| Msg::ClickTableauPile(pile_index));
                let fan = fan_offsets(pile);

                let cards = pile
                    .iter()
                    .zip(fan.cards.iter())
                    .enumerate()
                    .map(|(card_index, (tableau_card, steps))| {
                        let on_click = ctx.link().callback(move |event: MouseEvent| {
                            event.stop_propagation();
                            Msg::ClickTableauCard(pile_index, card_index)
                        });
                        let on_double_click = ctx.link().callback(move |event: MouseEvent| {
                            event.stop_propagation();
                            Msg::DoubleClickTableauCard(pile_index, card_index)
                        });

                        let selected = matches!(
                            self.game.selected,
                            Some(Selection::Tableau { pile, index })
                                if pile == pile_index && card_index >= index
                        );

                        let card_html = if tableau_card.face_up {
                            let origin = Selection::Tableau {
                                pile: pile_index,
                                index: card_index,
                            };
                            let away = self.is_away(tableau_card.card, &lifted);
                            let pointer = Self::pointer_callbacks(ctx, origin);
                            self.view_face_card(
                                tableau_card.card,
                                selected,
                                tableau_card.zeus_revealed,
                                away,
                                on_click,
                                on_double_click,
                                pointer,
                                None,
                                false,
                            )
                        } else {
                            let block_click = ctx.link().callback(|event: MouseEvent| {
                                event.stop_propagation();
                                Msg::Noop
                            });
                            self.view_back_card(
                                tableau_card.card,
                                false,
                                "Hidden card",
                                block_click,
                                None,
                            )
                        };

                        // Keyed by true card identity — a flip between
                        // face-down and face-up, or a pile whose card count
                        // changes, never lets Yew reuse the wrong node.
                        html! {
                            <div
                                key={motion::card_key(tableau_card.card)}
                                class="tableau-layer"
                                style={format!(
                                    "--down-steps: {}; --up-steps: {};",
                                    steps.down, steps.up,
                                )}
                            > { card_html } </div>
                        }
                    })
                    .collect::<Html>();

                let mut pile_classes = classes!("tableau-pile");
                if pile.is_empty() {
                    pile_classes.push("empty");
                }
                if self.hover_target == Some(DropTarget::Tableau(pile_index)) {
                    pile_classes.push("drop-target");
                }

                html! {
                    <div class="tableau-column">
                        <div class="pile-label">{ format!("Column {}", pile_index + 1) }</div>
                        <div
                            class={pile_classes}
                            data-drop-tableau={pile_index.to_string()}
                            onclick={
                                if locked {
                                    ctx.link().callback(|_| Msg::Noop)
                                } else {
                                    pile_click
                                }
                            }
                            style={format!(
                                "--down-steps: {}; --up-steps: {};",
                                fan.pile.down, fan.pile.up,
                            )}
                            aria-label={format!("Tableau column {}", pile_index + 1)}
                        >
                            { cards }
                        </div>
                    </div>
                }
            })
            .collect::<Html>();

        let victory_rain_active =
            matches!(self.end_state, Some(EndState::Victory)) && !self.victory_rain_dismissed;
        let victory_gold_animation = if victory_rain_active {
            (0..18)
                .map(|idx| {
                    html! {
                        <span class="victory-coin" style={format!("--coin-index: {idx};")}></span>
                    }
                })
                .collect::<Html>()
        } else {
            Html::default()
        };
        let dismiss_victory_rain = ctx.link().callback(|_| Msg::DismissVictoryRain);

        html! {
            <main class={classes!(
                "app-shell",
                self.end_state.is_some().then_some("ended"),
                self.end_state.is_some_and(EndState::is_loss).then_some("lost"),
                matches!(self.end_state, Some(EndState::ZeusThunder)).then_some("thunder-ended"),
                matches!(self.end_state, Some(EndState::OutOfGold)).then_some("gold-ended"),
                matches!(self.end_state, Some(EndState::Stalemate)).then_some("stalemate-ended"),
                matches!(self.end_state, Some(EndState::Victory)).then_some("victory-ended"),
            )} onclick={dismiss_victory_rain}>
                <div class="victory-coins" aria-hidden="true">{ victory_gold_animation }</div>
                <div class="felt-art" aria-hidden="true"></div>
                <div class="host-nymphs" aria-hidden="true">
                    <span class={classes!("host-nymph", "left", "art-nymph-blonde")}></span>
                    <span class={classes!("host-nymph", "right", "art-nymph-brunette")}></span>
                </div>
                <div class={classes!("victory-temple", "art-temple-with-coin")} aria-hidden="true"></div>
                <header class="title-wrap">
                    <div class="title-art" aria-hidden="true">
                        <span class={classes!("title-medallion", "art-laurel")}></span>
                        <span class={classes!("title-medallion", "art-dionysus")}></span>
                        <span class={classes!("title-medallion", "art-temple")}></span>
                    </div>
                    <h1>{ "Solitare of Olympus" }</h1>
                    <p>{ "Play cards with Cupid, ivy, Bacchus, and temple gold." }</p>
                </header>

                <section class="control-row">
                    <button type="button" onclick={new_game}>{ reset_label }</button>
                    <button type="button" onclick={switch_draw_mode} disabled={locked || actions_busy || self.game.moves > 0}>
                        { if easy_mode_active { "Switch To Hard" } else { "Switch To Easy" } }
                    </button>
                    <button type="button" onclick={auto_foundation} disabled={locked || actions_busy}>{ "Auto To Temple" }</button>
                    <button type="button" onclick={all_to_temple} disabled={locked || actions_busy}>{ "All To Temple" }</button>
                    <button type="button" onclick={zeus_vision} disabled={locked || actions_busy}>{ "Zeus' Vision" }</button>
                    <button type="button" class="help-toggle-btn" onclick={toggle_help}>{ help_button_label }</button>
                </section>

                <section class="status-row">
                    <div class="status-pill">{ format!("Moves: {}", self.game.moves) }</div>
                    <div class="status-pill">{ format!("Temple Gold: {}", self.game.temple_gold) }</div>
                    <div class="status-pill">{ format!("Mode: {}", mode_label) }</div>
                    <div class="status-text">{ &self.status }</div>
                </section>

                <section class="top-board">
                    <div class="pile-slot">
                        <div class="pile-label">{ "Stock" }</div>
                        { stock_view }
                    </div>
                    <div class="pile-slot">
                        <div class="pile-label">{ "Waste" }</div>
                        { waste_view }
                    </div>
                    <div class="pile-slot top-board-gap" aria-hidden="true"></div>
                    { foundation_slots }
                </section>

                <section class="tableau-scroll">
                    <div class="tableau-grid">
                        { tableau_columns }
                    </div>
                </section>

                <section class={classes!("help-strip", (!self.help_expanded).then_some("collapsed"))}>
                    <span>{ "Click to select and move, or drag a card straight to its destination." }</span>
                    <span>{ "Double-click waste/top tableau card to send it to a temple." }</span>
                    <span>{ "Build tableau in descending alternating colors." }</span>
                    <span>{ "Zeus' Vision reveals hidden cards and ends the game." }</span>
                    <span>{ "All To Temple auto-runs endgame moves until no temple move remains." }</span>
                    <span>{ "Keys: D or Enter draws, Space sends one to temple, A sends all." }</span>
                </section>
                <span class="version-tag" aria-hidden="true">{ concat!("v", env!("CARGO_PKG_VERSION")) }</span>
                { self.view_flight_layer() }
                { self.view_drag_overlay() }
            </main>
        }
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    info!("Starting Solitare of Olympus");
    Renderer::<App>::new().render();
}

#[cfg(test)]
mod tests {
    use super::{
        App, CardSteps, DragPhase, DropTarget, EMPTY_TABLEAU_NEEDS_KING, ILLEGAL_FOUNDATION_MOVE,
        ILLEGAL_TABLEAU_MOVE, PointerPoint, advance_drag_phase, exceeds_tap_threshold, fan_offsets,
    };
    use solitare::game::{Card, GameState, Selection, Suit, TableauCard};

    fn card(rank: u8, face_up: bool) -> TableauCard {
        TableauCard {
            card: Card {
                suit: Suit::Spades,
                rank,
            },
            face_up,
            zeus_revealed: false,
        }
    }

    fn mixed_pile() -> Vec<TableauCard> {
        vec![
            card(1, false),
            card(2, false),
            card(3, true),
            card(4, true),
            card(5, true),
        ]
    }

    #[test]
    fn fan_offsets_counts_cards_below_each_card() {
        let fan = fan_offsets(&mixed_pile());

        assert_eq!(
            fan.cards,
            vec![
                CardSteps { down: 0, up: 0 },
                CardSteps { down: 1, up: 0 },
                CardSteps { down: 2, up: 0 },
                CardSteps { down: 2, up: 1 },
                CardSteps { down: 2, up: 2 },
            ]
        );
    }

    #[test]
    fn fan_offsets_pile_matches_last_card() {
        let fan = fan_offsets(&mixed_pile());

        assert_eq!(fan.pile, CardSteps { down: 2, up: 2 });
    }

    #[test]
    fn fan_offsets_empty_column_has_zero_steps() {
        let pile: Vec<TableauCard> = vec![];

        let fan = fan_offsets(&pile);

        assert!(fan.cards.is_empty());
        assert_eq!(fan.pile, CardSteps { down: 0, up: 0 });
    }

    fn point(x: f64, y: f64) -> PointerPoint {
        PointerPoint { x, y }
    }

    #[test]
    fn two_pixels_of_travel_is_a_tap() {
        let start = point(0.0, 0.0);
        assert!(!exceeds_tap_threshold(start, point(2.0, 0.0)));
    }

    #[test]
    fn exactly_three_pixels_of_travel_is_still_a_tap() {
        let start = point(0.0, 0.0);
        assert!(!exceeds_tap_threshold(start, point(3.0, 0.0)));
    }

    #[test]
    fn six_pixels_of_travel_is_a_drag() {
        let start = point(0.0, 0.0);
        assert!(exceeds_tap_threshold(start, point(6.0, 0.0)));
    }

    #[test]
    fn diagonal_travel_uses_straight_line_distance() {
        // 3px right and 3px down is well past 3px of straight-line travel,
        // even though each axis alone would read as a tap.
        let start = point(0.0, 0.0);
        assert!(exceeds_tap_threshold(start, point(3.0, 3.0)));
    }

    #[test]
    fn drag_phase_stays_pressed_under_threshold() {
        let start = point(10.0, 10.0);
        let phase = advance_drag_phase(DragPhase::Pressed, start, point(11.0, 10.0));
        assert_eq!(phase, DragPhase::Pressed);
    }

    #[test]
    fn drag_phase_advances_past_threshold() {
        let start = point(10.0, 10.0);
        let phase = advance_drag_phase(DragPhase::Pressed, start, point(20.0, 10.0));
        assert_eq!(phase, DragPhase::Dragging);
    }

    #[test]
    fn drag_phase_never_reverts_to_pressed() {
        let start = point(10.0, 10.0);
        // Once dragging, drifting back within 3px of the start must not
        // resurrect tap behavior mid-gesture.
        let phase = advance_drag_phase(DragPhase::Dragging, start, point(10.5, 10.0));
        assert_eq!(phase, DragPhase::Dragging);
    }

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

    fn app_with(game: GameState) -> App {
        App {
            game,
            status: String::new(),
            end_state: None,
            help_expanded: false,
            victory_gold_award: 0,
            victory_rain_dismissed: false,
            all_to_temple_running: false,
            all_to_temple_timeout: None,
            key_listener: None,
            drag: None,
            just_dragged: false,
            hover_target: None,
            flights: Vec::new(),
        }
    }

    #[test]
    fn resolve_drop_onto_empty_tableau_column_requires_a_king() {
        let mut game = GameState::empty();
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        let outcome = app.resolve_drop(Some(DropTarget::Tableau(3)));

        assert!(!outcome.moved);
        assert_eq!(outcome.status, EMPTY_TABLEAU_NEEDS_KING);
    }

    #[test]
    fn resolve_drop_rejects_a_mismatched_tableau_stack() {
        let mut game = GameState::empty();
        game.tableau[0].push(card(9, true));
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        let outcome = app.resolve_drop(Some(DropTarget::Tableau(0)));

        assert!(!outcome.moved);
        assert_eq!(outcome.status, ILLEGAL_TABLEAU_MOVE);
    }

    #[test]
    fn click_tableau_pile_rejects_a_mismatched_tableau_stack() {
        let mut game = GameState::empty();
        game.tableau[0].push(card(9, true));
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        app.click_tableau_pile(None, 0);

        assert_eq!(app.status, ILLEGAL_TABLEAU_MOVE);
        assert_eq!(app.game.tableau[0].len(), 1);
        assert_eq!(app.game.waste.len(), 1);
        assert_eq!(app.game.selected, Some(Selection::Waste));
    }

    #[test]
    fn click_tableau_pile_onto_empty_column_requires_a_king() {
        let mut game = GameState::empty();
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        app.click_tableau_pile(None, 3);

        assert_eq!(app.status, EMPTY_TABLEAU_NEEDS_KING);
        assert_eq!(app.game.waste.len(), 1);
    }

    #[test]
    fn resolve_drop_places_a_legal_tableau_run_and_words_it_like_a_click() {
        let mut game = GameState::empty();
        game.tableau[1].push(TableauCard {
            card: heart(6),
            face_up: true,
            zeus_revealed: false,
        });
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        let outcome = app.resolve_drop(Some(DropTarget::Tableau(1)));

        assert!(outcome.moved);
        assert_eq!(outcome.status, "Moved cards to tableau column 2.");
    }

    #[test]
    fn resolve_drop_rejects_a_non_ace_onto_an_empty_foundation() {
        let mut game = GameState::empty();
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        let outcome = app.resolve_drop(Some(DropTarget::Foundation(0)));

        assert!(!outcome.moved);
        assert_eq!(outcome.status, ILLEGAL_FOUNDATION_MOVE);
    }

    #[test]
    fn resolve_drop_places_an_ace_and_words_it_like_a_click() {
        let mut game = GameState::empty();
        game.waste.push(spade(1));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        let outcome = app.resolve_drop(Some(DropTarget::Foundation(0)));

        assert!(outcome.moved);
        assert_eq!(outcome.status, "Placed card on foundation 1.");
    }

    #[test]
    fn resolve_drop_with_no_target_is_an_illegal_tableau_move() {
        let mut game = GameState::empty();
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        let outcome = app.resolve_drop(None);

        assert!(!outcome.moved);
        assert_eq!(outcome.status, ILLEGAL_TABLEAU_MOVE);
    }
}
