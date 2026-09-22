mod motion;

use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use log::info;
use motion::{Flight, FlightKind, Rect};
use solitare::game::{Card, EASY_DRAW_COUNT, GameState, HARD_DRAW_COUNT, Selection, TableauCard};
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent as DomKeyboardEvent;
use yew::events::{MouseEvent, PointerEvent, TransitionEvent};
use yew::{Classes, Component, Context, Html, Renderer, classes, create_portal, html};

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

/// What a `Msg::DrawStock` actually did, for its arm to word and to check
/// for the out-of-gold ending: whether the stock had a card to draw or the
/// waste one to recycle, and how many cards landed in the waste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrawOutcome {
    had_stock: bool,
    had_waste: bool,
    drawn: usize,
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

/// The page's current scroll offset, read fresh at every rect capture so a
/// scroll between two captures never leaves a stored rect stale.
fn scroll_offset() -> (f64, f64) {
    let Some(window) = web_sys::window() else {
        return (0.0, 0.0);
    };
    (
        window.scroll_x().unwrap_or(0.0),
        window.scroll_y().unwrap_or(0.0),
    )
}

/// A pressed card's on-screen box: its live flight-layer position when it
/// is still in flight, or its own (board) box otherwise. A card pressed
/// mid-flight is visible where the flight drew it, not at its hidden board
/// slot, so the grab point must come from there.
fn grab_source_rect(card: &web_sys::Element) -> Rect {
    if let Some(id) = card.get_attribute("data-card-id")
        && let Some(in_flight) = element_rect(&format!("[data-flight-card='{id}']"))
    {
        return in_flight;
    }
    let rect = card.get_bounding_client_rect();
    Rect {
        x: rect.x(),
        y: rect.y(),
        width: rect.width(),
        height: rect.height(),
    }
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

/// An empty waste or foundation slot's placeholder label, shared by the
/// slot's own empty-state button and its underlay (shown while its last
/// card is away on a flight).
const WASTE_EMPTY_LABEL: (&str, &str) = ("WASTE", "DRAW");
const TEMPLE_EMPTY_LABEL: (&str, &str) = ("TEMPLE", "ACE UP");

/// A flying or dragged card's "lifted" shadow, and the resting shadow it
/// eases to by the time it lands — matching `.card, .pile-empty` in
/// style.css so landing changes nothing visible.
const LIFTED_SHADOW: &str = "0 16px 24px rgba(0, 0, 0, 0.4)";
const RESTING_SHADOW: &str = "0 8px 12px rgba(0, 0, 0, 0.2)";

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
    /// Set by a card-moving message, so `rendered()` measures the board
    /// only then; a `PointerMove` or `ToggleHelp` render never sets it, so
    /// neither ever pays for a `getBoundingClientRect` call.
    measure_pending: bool,
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
    /// A flight's live position and new destination, measured once its
    /// board copy has rendered. `f64` is the flight's `launched_at` at
    /// measurement time, guarding against a flight that was replaced
    /// between the measurement and this message's delivery.
    FlightDestinationsMeasured(Vec<(Card, f64, Rect, Rect)>),
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
            // own rect (or, mid-flight, the flight layer's), so the overlay
            // can keep this exact point under the pointer instead of
            // centering the card on it. `target()`, not `current_target()`
            // — Yew delegates events to a shared root, so `current_target`
            // is that root, not the button; `target()` may land on an
            // inner span, so `closest` walks up to the card.
            let grab_offset = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                .and_then(|element| element.closest(".card").ok().flatten())
                .map(|card| grab_source_rect(&card))
                .map(|rect| PointerPoint {
                    x: at.x - rect.x,
                    y: at.y - rect.y,
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

    /// The flights a drag's release plans: `moved` selects Travel or
    /// SettleBack, the same choice a cancel always makes for itself. Ctx-free
    /// so a host test can drive the landed and rejected cases directly,
    /// through the code `PointerUp` and `PointerCancel` call.
    fn plan_release_flights(
        dragged: &[Card],
        departures: &[(Card, Rect)],
        moved: bool,
        launched_at: f64,
    ) -> Vec<Flight> {
        let kind = if moved {
            FlightKind::Travel
        } else {
            FlightKind::SettleBack
        };
        motion::plan_drag_flights(dragged, departures, kind, launched_at)
    }

    /// The dragged cards and their departure rects at a release: read
    /// before anything moves them, since a landed drop's own mutation would
    /// otherwise change what `dragged_cards(origin)` finds there. Shared by
    /// a drop and a cancel, which diverge only after this capture — a drop
    /// still has to resolve against its target before it knows whether it
    /// moved.
    fn capture_release(
        &self,
        origin: Selection,
        reduced_motion: bool,
    ) -> (Vec<Card>, Vec<(Card, Rect)>) {
        let dragged = self.dragged_cards(origin);
        let departures = self.capture_overlay_departure_rects(&dragged, reduced_motion);
        (dragged, departures)
    }

    /// Undoes a cancelled drag: the selection returns to what it was before
    /// the press, the drop highlight clears and the dragged cards settle
    /// home. Ctx-free so a host test drives the code `PointerCancel` runs.
    fn cancel_release(
        &mut self,
        tracker: &DragTracker,
        dragged: &[Card],
        departures: &[(Card, Rect)],
        now: f64,
    ) -> Vec<Flight> {
        self.game.selected = tracker.selection_before;
        self.hover_target = None;
        Self::plan_release_flights(dragged, departures, false, now)
    }

    /// Plans and launches a landed or rejected drop's flight: Travel when
    /// the drop moved the cards, SettleBack when it did not.
    fn launch_release(
        &mut self,
        ctx: &Context<Self>,
        dragged: &[Card],
        departures: &[(Card, Rect)],
        moved: bool,
    ) {
        let flights = Self::plan_release_flights(dragged, departures, moved, js_sys::Date::now());
        self.launch_flights(ctx, flights);
    }

    /// Resolves a tap on tableau `pile`: extends the selection onto it, or
    /// without a selection, selects its top face-up card. Words a rejected
    /// move the same way `resolve_drop` does. Pure GameState-and-status
    /// logic — the caller plans and launches any flight the move needs, so
    /// this stays callable from a hermetic test with no Yew context.
    fn click_tableau_pile(&mut self, pile: usize) {
        if self.game.selected.is_some() {
            if self.game.move_selected_to_tableau(pile) {
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

    /// Starts a drag once the gesture crosses the tap threshold. The overlay
    /// becomes the dragged cards' only visible copy, so a flight still
    /// carrying one of them retires. The origin is selected unless it
    /// already is: the select_* calls toggle an active selection off.
    fn lift_drag(&mut self, origin: Selection) {
        let dragged = self.dragged_cards(origin);
        self.flights
            .retain(|flight| !dragged.contains(&flight.card));
        if self.game.is_selected(origin) {
            return;
        }
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

    /// The stock slot's departure rect for a draw, in page coordinates: a
    /// flight stores every rect it holds in that frame (`motion::Flight`).
    fn rect_for_stock_slot() -> Option<Rect> {
        let (scroll_x, scroll_y) = scroll_offset();
        element_rect("[data-pile-slot='stock']")
            .map(|rect| motion::to_page_rect(rect, scroll_x, scroll_y))
    }

    /// Every currently visible face-up card's departure rect, read from the
    /// DOM before `mutate` changes `GameState` and converted to page
    /// coordinates: a card already mid-flight departs from its live
    /// flight-layer position, not its stale slot. Empty under reduced
    /// motion, so callers never need their own guard.
    fn capture_departure_rects(
        &self,
        before: &motion::Snapshot,
        reduced_motion: bool,
    ) -> Vec<(Card, Rect)> {
        if reduced_motion {
            return Vec::new();
        }
        let (scroll_x, scroll_y) = scroll_offset();
        before
            .iter()
            .filter_map(|(card, _)| {
                let key = motion::card_key(*card);
                let board = element_rect(&format!("[data-card-id='{key}']"))
                    .map(|rect| motion::to_page_rect(rect, scroll_x, scroll_y));
                let in_flight = self
                    .is_flying(*card)
                    .then(|| element_rect(&format!("[data-flight-card='{key}']")))
                    .flatten()
                    .map(|rect| motion::to_page_rect(rect, scroll_x, scroll_y));
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

    /// A dragged card's departure rect at release, in page coordinates:
    /// wherever the overlay last drew it, read from the DOM before the
    /// overlay is torn down. Empty under reduced motion, so callers never
    /// need their own guard.
    fn capture_overlay_departure_rects(
        &self,
        dragged: &[Card],
        reduced_motion: bool,
    ) -> Vec<(Card, Rect)> {
        if reduced_motion {
            return Vec::new();
        }
        let (scroll_x, scroll_y) = scroll_offset();
        dragged
            .iter()
            .filter_map(|card| {
                let selector = format!("[data-overlay-card='{}']", motion::card_key(*card));
                element_rect(&selector)
                    .map(|rect| (*card, motion::to_page_rect(rect, scroll_x, scroll_y)))
            })
            .collect()
    }

    /// Runs `mutate` against the whole component, then plans and launches
    /// flights for every card the before/after snapshot diff calls moved.
    /// The one primitive every move path shares: `apply_move_with_flight`
    /// wraps it for the common case of a single `GameState`-mutating call;
    /// `Msg::ClickTableauPile`'s handler calls it directly, since
    /// `click_tableau_pile`'s two-branch logic sets `status` itself.
    fn with_flights(&mut self, ctx: &Context<Self>, mutate: impl FnOnce(&mut Self)) {
        self.measure_pending = true;
        let reduced_motion = prefers_reduced_motion();
        let before = motion::snapshot(&self.game);
        let departures = self.capture_departure_rects(&before, reduced_motion);
        mutate(self);
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

    /// Applies one GameState-mutating move through `with_flights`, wrapping
    /// `move_selected_to_tableau`/`_foundation` and `auto_promote_lowest`
    /// identically so every click, double-click and auto-promotion path
    /// gains motion from one call and still reports whether it moved.
    fn apply_move_with_flight<F>(&mut self, ctx: &Context<Self>, mutate: F) -> bool
    where
        F: FnOnce(&mut GameState) -> bool,
    {
        let mut moved = false;
        self.with_flights(ctx, |app| moved = mutate(&mut app.game));
        moved
    }

    /// Clears every flight in the air: a fresh deal, a stock recycle or
    /// Zeus' Vision replaces or re-fans the whole board, so nothing already
    /// under way corresponds to where its flight was headed.
    fn reset_flights(&mut self) {
        self.flights.clear();
    }

    /// Resets session state onto a freshly dealt game, flights included.
    /// `dealt` is a parameter, not a call to `GameState::new_shuffled_with_
    /// draw_count` here, so a host test can supply a deterministic board
    /// instead of a real shuffle.
    fn reset_for_new_game(&mut self, dealt: GameState, carry_gold: usize) {
        self.game = dealt;
        self.game.temple_gold = carry_gold;
        self.stop_all_to_temple();
        self.end_state = None;
        self.victory_gold_award = 0;
        self.victory_rain_dismissed = false;
        self.status = "You gave up. A fresh deck has been dealt.".to_string();
        self.reset_flights();
        self.measure_pending = true;
    }

    /// Words a recycle and clears any flight in the air: with the waste
    /// shuffled back into the stock, no card already travelling corresponds
    /// to where it was headed.
    fn recycle_status(&mut self, gold_before: usize) -> String {
        self.reset_flights();
        let collected = gold_before.saturating_sub(self.game.temple_gold);
        if collected > 0 {
            format!("Recycled waste back into stock. Temple collected {collected} gold.")
        } else {
            "Recycled waste back into stock.".to_string()
        }
    }

    /// Appends each flight and arms its deadline: a flight always lands,
    /// even when its `transitionend` never fires (an element removed, a
    /// tab hidden, reduced motion switched on mid-flight).
    fn launch_flights(&mut self, ctx: &Context<Self>, flights: Vec<Flight>) {
        for flight in flights {
            // A card moved again mid-flight supersedes its own earlier
            // flight rather than flying twice at once; the old timer still
            // fires later, but by then this card's launched_at has moved
            // on, so `ExpireFlight` finds nothing to remove.
            self.flights.retain(|existing| existing.card != flight.card);
            let deadline_ms = (flight.deadline() - flight.launched_at).max(0.0) as u32;
            Self::arm_expiry(ctx, flight.card, flight.launched_at, deadline_ms);
            self.flights.push(flight);
        }
    }

    /// Schedules a flight's fallback landing at `deadline_ms` from
    /// `launched_at`: a flight always lands even without a `transitionend`.
    /// Shared by a fresh launch and a re-aim, which restarts this clock
    /// from the moment it re-aims rather than the flight's original launch.
    fn arm_expiry(ctx: &Context<Self>, card: Card, launched_at: f64, deadline_ms: u32) {
        let link = ctx.link().clone();
        Timeout::new(deadline_ms, move || {
            link.send_message(Msg::ExpireFlight(card, launched_at));
        })
        .forget();
    }

    /// Plans and launches the new waste top's flight from the stock slot,
    /// covering any lower cards a multi-draw reveals silently beneath it.
    fn launch_draw_flight(
        &mut self,
        ctx: &Context<Self>,
        stock_rect: Rect,
        drawn: usize,
        reduced_motion: bool,
    ) {
        let new_top = self.game.waste.last().copied();
        let launched_at = js_sys::Date::now();
        let Some(flight) =
            motion::plan_draw_flight(new_top, stock_rect, reduced_motion, launched_at)
        else {
            return;
        };
        // Draw-3's lower two cards arrive silently: only the true top gets
        // a flight, so the underlay must skip them too until it lands.
        let covers = if drawn > 1 {
            let waste_len = self.game.waste.len();
            self.game.waste[waste_len - drawn..waste_len - 1].to_vec()
        } else {
            Vec::new()
        };
        self.launch_flights(ctx, vec![flight.with_covers(covers)]);
    }

    /// Draws or recycles the stock and launches the new waste top's flight:
    /// the reduced-motion check, the stock's pre-draw rect and the drawn
    /// count each live here once instead of twice across `Msg::DrawStock`.
    fn draw_stock(&mut self, ctx: &Context<Self>) -> DrawOutcome {
        let had_stock = !self.game.stock.is_empty();
        let had_waste = !self.game.waste.is_empty();
        let waste_before = self.game.waste.len();
        let reduced_motion = prefers_reduced_motion();
        // The new waste top departs from the stock slot, already face up:
        // measure before the draw pops the stock's card.
        let stock_rect = (had_stock && !reduced_motion)
            .then(Self::rect_for_stock_slot)
            .flatten();

        self.game.draw_or_recycle();
        let drawn = self.game.waste.len().saturating_sub(waste_before);

        if let Some(stock_rect) = stock_rect {
            self.launch_draw_flight(ctx, stock_rect, drawn, reduced_motion);
        }

        DrawOutcome {
            had_stock,
            had_waste,
            drawn,
        }
    }

    /// Measures every flight's current destination and, for one whose
    /// destination has moved (unmeasured yet, or re-fanned since), reports
    /// it along with the flight's own live position. Called from
    /// `rendered()`, after the render that moved it has already painted —
    /// a destination that has not changed is skipped, so an unrelated
    /// render costs nothing beyond the read. A never-aimed flight whose card
    /// has no element (buried under a later draw) lands at once; an aimed
    /// one keeps flying to the slot it aimed at.
    fn measure_flight_destinations(&self, ctx: &Context<Self>) {
        let (scroll_x, scroll_y) = scroll_offset();
        let mut measured: Vec<(Card, f64, Rect, Rect)> = Vec::new();
        for flight in &self.flights {
            let selector = format!("[data-card-id='{}']", motion::card_key(flight.card));
            let destination =
                element_rect(&selector).map(|rect| motion::to_page_rect(rect, scroll_x, scroll_y));
            let live_selector = format!("[data-flight-card='{}']", motion::card_key(flight.card));
            let live = element_rect(&live_selector)
                .map(|rect| motion::to_page_rect(rect, scroll_x, scroll_y));
            match motion::resolve_flight_measurement(flight, destination, live) {
                motion::FlightMeasurement::Vanished => {
                    ctx.link()
                        .send_message(Msg::ExpireFlight(flight.card, flight.launched_at));
                }
                motion::FlightMeasurement::Moved { live, destination } => {
                    measured.push((flight.card, flight.launched_at, live, destination));
                }
                motion::FlightMeasurement::Unchanged => {}
            }
        }
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

    /// Every card in flight, each its own absolutely positioned layer so it
    /// can travel to its own measured destination independently — a run
    /// flies as a run, but each card keeps its own fan slot. Portalled into
    /// `<body>`, outside `.app-shell`'s clip, and positioned in page
    /// coordinates so a scroll during the flight moves it and its slot
    /// together instead of needing a re-measure. Its shadow eases from the
    /// lifted look to the board card's resting shadow over the same
    /// transition as its transform, so it lands unchanged; landing on
    /// `transitionend` is the fast path, the deadline sweep the fallback.
    fn view_flight_layer(&self, ctx: &Context<Self>) -> Html {
        let layer = self
            .flights
            .iter()
            .map(|flight| {
                let rect = flight.to.unwrap_or(flight.from);
                let mut classes = classes!("card", "face", "flight-card");
                classes.extend(Self::card_palette_classes(flight.card));
                if flight.kind == FlightKind::SettleBack {
                    classes.push("settle-back");
                }
                let shadow = if flight.to.is_some() {
                    RESTING_SHADOW
                } else {
                    LIFTED_SHADOW
                };
                let style = format!(
                    "transform: translate({}px, {}px); box-shadow: {shadow};",
                    rect.x, rect.y
                );
                let card_id = motion::card_key(flight.card);
                let card = flight.card;
                let launched_at = flight.launched_at;
                let on_transition_end = ctx.link().callback(move |event: TransitionEvent| {
                    if event.property_name() == "transform" {
                        Msg::ExpireFlight(card, launched_at)
                    } else {
                        Msg::Noop
                    }
                });
                html! {
                    <div
                        key={format!("flight-{card_id}")}
                        class={classes}
                        style={style}
                        aria-hidden="true"
                        data-flight-card={card_id}
                        ontransitionend={on_transition_end}
                    >
                        { Self::card_face_content(flight.card) }
                    </div>
                }
            })
            .collect::<Html>();
        create_portal(layer, Self::flight_layer_host())
    }

    /// The flight layer's portal target: `<body>` itself, which sits above
    /// no positioned ancestor, so a flight card's `position: absolute`
    /// places it in page coordinates and nothing clips it. Every document
    /// has a body once the app has mounted.
    fn flight_layer_host() -> web_sys::Element {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.query_selector("body").ok().flatten())
            .expect("the document has a <body> once the app has mounted")
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

    /// `key` is the slot's Yew list key, not the card's own identity: the
    /// waste and foundation top buttons key by slot ("waste",
    /// "foundation-0") so a reused DOM node keeps keyboard focus when its
    /// top card changes, as `main` does; the tableau keys by card, since a
    /// column's cards each need their own stable node. Hiding is a
    /// Yew-managed class (`away`), so a reused node still hides the right
    /// card.
    #[allow(clippy::too_many_arguments)]
    fn view_face_card(
        &self,
        card: Card,
        key: String,
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
            // by a drag must stay hit-testable and clickable.
            card_classes.push("away");
        }
        let card_id = motion::card_key(card);

        html! {
            <button
                type="button"
                key={key}
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

    /// `key` per `view_face_card`'s note: the stock keys by "stock", a
    /// stable slot key, so a draw's new top reuses the same node and keeps
    /// keyboard focus; a face-down tableau card keys by its own identity.
    fn view_back_card(
        &self,
        key: String,
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
                key={key}
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

    /// `key` per `view_face_card`'s note, applied to the button in both the
    /// filled and empty forms and wrapped in the same `card-frame` either
    /// way: Yew reuses one DOM node across a slot's own empty ↔ filled
    /// transitions only when both the wrapper and the button match on
    /// every render, so keyboard focus on the slot survives it.
    fn view_foundation_slot(&self, ctx: &Context<Self>, pile: usize, lifted: &[Card]) -> Html {
        let on_click = ctx.link().callback(move |_| Msg::ClickFoundation(pile));
        let selected = self.game.is_selected(Selection::Foundation { pile });
        let drop_target = self.hover_target == Some(DropTarget::Foundation(pile));
        let foundation = &self.game.foundations[pile];
        let key = format!("foundation-{pile}");

        let (button, underlay) = if let Some(card) = foundation.last().copied() {
            let top_away = self.is_away(card, lifted);
            let underlay = top_away
                .then(|| Self::view_pile_underlay(foundation, &self.flights, TEMPLE_EMPTY_LABEL));
            let on_double_click = ctx.link().callback(|_| Msg::Noop);
            let pointer = Self::pointer_callbacks(ctx, Selection::Foundation { pile });
            let button = self.view_face_card(
                card,
                key,
                selected,
                false,
                top_away,
                on_click,
                on_double_click,
                pointer,
                Some(pile),
                drop_target,
            );
            (button, underlay)
        } else {
            let button = html! {
                <button
                    type="button"
                    key={key}
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
                    <span>{ TEMPLE_EMPTY_LABEL.0 }</span>
                    <span class="tiny">{ TEMPLE_EMPTY_LABEL.1 }</span>
                </button>
            };
            (button, None)
        };

        html! {
            <div class="card-frame">
                { for underlay }
                { button }
            </div>
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
            measure_pending: false,
        }
    }

    fn rendered(&mut self, ctx: &Context<Self>, first_render: bool) {
        // Drops a flight already past its deadline. This alone repaints
        // nothing — `rendered` cannot trigger one — it only keeps
        // `self.flights` from outliving its own `ExpireFlight` timeout,
        // whose message is what actually lands a flight and repaints.
        let now = js_sys::Date::now();
        self.flights.retain(|flight| !flight.has_expired(now));

        // A flight's destination can only be measured once its (hidden)
        // board copy has actually painted, and a card still in the air may
        // need re-aiming toward a destination that has since moved — but
        // only a card-moving message can have changed either, so a render
        // that did not set `measure_pending` skips the read entirely.
        if self.measure_pending {
            self.measure_pending = false;
            self.measure_flight_destinations(ctx);
        }

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
                let dealt = GameState::new_shuffled_with_draw_count(self.game.draw_count);
                self.reset_for_new_game(dealt, carry_gold);
            }
            Msg::DrawStock => {
                self.measure_pending = true;
                let gold_before = self.game.temple_gold;
                let outcome = self.draw_stock(ctx);
                self.status = if outcome.had_stock {
                    let suffix = if outcome.drawn == 1 { "" } else { "s" };
                    format!("Drew {} card{suffix} to the waste pile.", outcome.drawn)
                } else if outcome.had_waste {
                    self.recycle_status(gold_before)
                } else {
                    "No cards available to draw.".to_string()
                };

                if !outcome.had_stock && outcome.had_waste && self.game.temple_gold == 0 {
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
                    if self
                        .apply_move_with_flight(ctx, |game| game.move_selected_to_any_foundation())
                    {
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
                    if self
                        .apply_move_with_flight(ctx, |game| game.move_selected_to_foundation(pile))
                    {
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
                    if self.apply_move_with_flight(ctx, |game| game.move_selected_to_tableau(pile))
                    {
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
                    if self
                        .apply_move_with_flight(ctx, |game| game.move_selected_to_any_foundation())
                    {
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
                // Guarded like the other three Click* handlers: inert today
                // only because cards stop_propagation() and pointer capture
                // retarget the trailing click away from the pile — accident,
                // not a rule this handler can rely on.
                if self.just_dragged {
                    return false;
                }
                self.with_flights(ctx, |app| app.click_tableau_pile(pile));
            }
            Msg::AutoFoundation => {
                if self.apply_move_with_flight(ctx, |game| game.auto_promote_lowest()) {
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
                if self.apply_move_with_flight(ctx, |game| game.auto_promote_lowest()) {
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

                if self.apply_move_with_flight(ctx, |game| game.auto_promote_lowest()) {
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
                self.reset_flights();
                self.measure_pending = true;
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

                if just_started {
                    self.lift_drag(origin);
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
                self.measure_pending = true;
                let (dragged, departures) =
                    self.capture_release(tracker.origin, prefers_reduced_motion());
                let target = Self::hit_test_drop_target(x, y);
                let outcome = self.resolve_drop(target);
                self.just_dragged = true;
                Self::schedule_just_dragged_clear(ctx);
                if !outcome.moved {
                    self.game.selected = tracker.selection_before;
                }
                self.launch_release(ctx, &dragged, &departures, outcome.moved);
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
                self.measure_pending = true;
                let (dragged, departures) =
                    self.capture_release(tracker.origin, prefers_reduced_motion());
                let flights =
                    self.cancel_release(&tracker, &dragged, &departures, js_sys::Date::now());
                self.launch_flights(ctx, flights);
            }
            Msg::ClearJustDragged => {
                self.just_dragged = false;
                return false;
            }
            Msg::FlightDestinationsMeasured(measurements) => {
                let now = js_sys::Date::now();
                for (card, guard_launched_at, live, destination) in measurements {
                    if let Some(flight) = self.flights.iter_mut().find(|flight| {
                        flight.card == card && flight.launched_at == guard_launched_at
                    }) {
                        motion::reaim(flight, live, destination, now);
                        let deadline_ms = (flight.deadline() - now).max(0.0) as u32;
                        Self::arm_expiry(ctx, card, now, deadline_ms);
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

        let stock_view = match self.game.stock.last() {
            Some(_) => self.view_back_card(
                "stock".to_string(),
                false,
                "Draw from stock",
                draw_stock.clone(),
                Some("stock"),
            ),
            None => {
                let label = if self.game.waste.is_empty() {
                    "Stock"
                } else {
                    "Recycle waste"
                };
                html! {
                    <button type="button" key="stock" class="pile-empty stock-empty" onclick={draw_stock.clone()} aria-label={label} disabled={locked} data-pile-slot="stock">
                        <span>{ "REDEAL" }</span>
                        <span class="tiny">{ "STOCK" }</span>
                    </button>
                }
            }
        };

        // Same node-per-slot rule as `view_foundation_slot`: the button
        // stays keyed "waste" and wrapped in `card-frame` whether the slot
        // is filled or empty, so a reused node keeps keyboard focus.
        let (waste_button, waste_underlay) = if let Some(card) = self.game.waste.last().copied() {
            let selected = self.game.is_selected(Selection::Waste);
            let away = self.is_away(card, &lifted);
            let underlay = away.then(|| {
                Self::view_pile_underlay(&self.game.waste, &self.flights, WASTE_EMPTY_LABEL)
            });
            let pointer = Self::pointer_callbacks(ctx, Selection::Waste);
            let button = self.view_face_card(
                card,
                "waste".to_string(),
                selected,
                false,
                away,
                click_waste,
                double_click_waste,
                pointer,
                None,
                false,
            );
            (button, underlay)
        } else {
            let button = html! {
                <button
                    type="button"
                    key="waste"
                    class="pile-empty"
                    onclick={click_waste}
                    aria-label="Waste pile"
                    disabled={locked}
                >
                    <span>{ WASTE_EMPTY_LABEL.0 }</span>
                    <span class="tiny">{ WASTE_EMPTY_LABEL.1 }</span>
                </button>
            };
            (button, None)
        };
        let waste_view = html! {
            <div class="card-frame">
                { for waste_underlay }
                { waste_button }
            </div>
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
                                motion::card_key(tableau_card.card),
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
                                motion::card_key(tableau_card.card),
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
                { self.view_flight_layer(ctx) }
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
        App, CardSteps, DragPhase, DragTracker, DropTarget, EMPTY_TABLEAU_NEEDS_KING, Flight,
        FlightKind, ILLEGAL_FOUNDATION_MOVE, ILLEGAL_TABLEAU_MOVE, PointerPoint, Rect,
        advance_drag_phase, exceeds_tap_threshold, fan_offsets,
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
            measure_pending: false,
        }
    }

    #[test]
    fn reset_flights_clears_everything_in_the_air() {
        let mut app = app_with(GameState::empty());
        app.flights.push(Flight::new(
            spade(5),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 60.0,
                height: 85.0,
            },
            FlightKind::Travel,
            0.0,
        ));

        app.reset_flights();

        assert!(app.flights.is_empty());
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

        app.click_tableau_pile(0);

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

        app.click_tableau_pile(3);

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

    fn rect(x: f64, y: f64) -> Rect {
        Rect {
            x,
            y,
            width: 60.0,
            height: 85.0,
        }
    }

    #[test]
    fn plan_release_flights_travels_on_a_landed_drop() {
        let dragged = vec![spade(5)];
        let departures = vec![(spade(5), rect(1.0, 2.0))];

        let flights = App::plan_release_flights(&dragged, &departures, true, 0.0);

        assert_eq!(flights.len(), 1);
        assert_eq!(flights[0].kind, FlightKind::Travel);
    }

    #[test]
    fn plan_release_flights_settles_back_on_a_rejected_drop() {
        let dragged = vec![spade(5)];
        let departures = vec![(spade(5), rect(1.0, 2.0))];

        let flights = App::plan_release_flights(&dragged, &departures, false, 0.0);

        assert_eq!(flights.len(), 1);
        assert_eq!(flights[0].kind, FlightKind::SettleBack);
    }

    #[test]
    fn cancel_release_restores_the_selection_and_settles_home() {
        let mut game = GameState::empty();
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);
        app.hover_target = Some(DropTarget::Tableau(2));
        let tracker = DragTracker::new(
            1,
            Selection::Waste,
            None,
            PointerPoint { x: 0.0, y: 0.0 },
            PointerPoint { x: 0.0, y: 0.0 },
        );
        let dragged = vec![spade(5)];
        let departures = vec![(spade(5), rect(1.0, 2.0))];

        let flights = app.cancel_release(&tracker, &dragged, &departures, 0.0);

        assert_eq!(app.game.selected, None);
        assert_eq!(app.hover_target, None);
        assert_eq!(flights.len(), 1);
        assert_eq!(flights[0].kind, FlightKind::SettleBack);
    }

    #[test]
    fn lift_drag_retires_the_dragged_cards_flight_and_selects_the_origin() {
        let mut game = GameState::empty();
        game.tableau[0].push(TableauCard {
            card: spade(5),
            face_up: true,
            zeus_revealed: false,
        });
        let mut app = app_with(game);
        app.flights.push(Flight::new(
            spade(5),
            rect(0.0, 0.0),
            FlightKind::Travel,
            0.0,
        ));
        app.flights.push(Flight::new(
            spade(9),
            rect(0.0, 0.0),
            FlightKind::Travel,
            0.0,
        ));
        let origin = Selection::Tableau { pile: 0, index: 0 };

        app.lift_drag(origin);

        assert_eq!(app.flights.len(), 1);
        assert_eq!(app.flights[0].card, spade(9));
        assert_eq!(app.game.selected, Some(origin));
    }

    #[test]
    fn lift_drag_keeps_an_already_selected_origin_selected() {
        let mut game = GameState::empty();
        game.waste.push(spade(5));
        game.selected = Some(Selection::Waste);
        let mut app = app_with(game);

        app.lift_drag(Selection::Waste);

        assert_eq!(app.game.selected, Some(Selection::Waste));
    }

    #[test]
    fn reset_for_new_game_clears_flights_in_the_air() {
        let mut app = app_with(GameState::empty());
        app.flights.push(Flight::new(
            spade(5),
            rect(0.0, 0.0),
            FlightKind::Travel,
            0.0,
        ));

        app.reset_for_new_game(GameState::empty(), 0);

        assert!(app.flights.is_empty());
    }

    #[test]
    fn recycle_status_clears_flights_in_the_air() {
        let mut app = app_with(GameState::empty());
        app.flights.push(Flight::new(
            spade(5),
            rect(0.0, 0.0),
            FlightKind::Travel,
            0.0,
        ));

        let status = app.recycle_status(0);

        assert!(app.flights.is_empty());
        assert_eq!(status, "Recycled waste back into stock.");
    }
}
