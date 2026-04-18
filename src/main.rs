use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use log::info;
use solitare::game::{Card, EASY_DRAW_COUNT, GameState, HARD_DRAW_COUNT, Selection};
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent as DomKeyboardEvent;
use yew::events::MouseEvent;
use yew::{Component, Context, Html, Renderer, classes, html};

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
}

impl App {
    fn interactions_locked(&self) -> bool {
        self.end_state.is_some() || self.all_to_temple_running
    }

    fn describe_card(card: Card) -> String {
        format!("{}{}", card.rank_label(), card.suit.symbol())
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
        self.game.temple_gold = 0;
        self.victory_gold_award = reward;
        self.end_state = Some(EndState::Victory);
        self.stop_all_to_temple();
        self.status = format!("Dionysus honors you with {reward} gold.");
    }

    fn view_face_card(
        &self,
        card: Card,
        selected: bool,
        zeus_revealed: bool,
        on_click: yew::Callback<MouseEvent>,
        on_double_click: yew::Callback<MouseEvent>,
    ) -> Html {
        let mut card_classes = classes!("card", "face");
        card_classes.push(if card.is_red() { "red" } else { "black" });
        if selected {
            card_classes.push("selected");
        }
        if matches!(card.rank, 1 | 11 | 12 | 13) {
            card_classes.push("court");
        }
        if zeus_revealed {
            card_classes.push("zeus-revealed");
        }
        let center_art = if matches!(card.rank, 11..=13) {
            "art-dionysus"
        } else if card.rank == 1 {
            "art-temple"
        } else {
            "art-laurel"
        };

        html! {
            <button
                type="button"
                class={card_classes}
                onclick={on_click}
                ondblclick={on_double_click}
                aria-label={format!("{} of {}", card.rank_label(), card.suit.latin_name())}
                disabled={self.interactions_locked()}
            >
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
            </button>
        }
    }

    fn view_back_card(
        &self,
        selected: bool,
        label: &'static str,
        on_click: yew::Callback<MouseEvent>,
    ) -> Html {
        let mut card_classes = classes!("card", "back");
        if selected {
            card_classes.push("selected");
        }

        html! {
            <button
                type="button"
                class={card_classes}
                onclick={on_click}
                aria-label={label}
                disabled={self.interactions_locked()}
            >
                <span class="back-medallion" aria-hidden="true"></span>
            </button>
        }
    }

    fn view_foundation_slot(&self, ctx: &Context<Self>, pile: usize) -> Html {
        let on_click = ctx.link().callback(move |_| Msg::ClickFoundation(pile));
        let selected = self.game.is_selected(Selection::Foundation { pile });

        if let Some(card) = self.game.foundations[pile].last().copied() {
            let on_double_click = ctx.link().callback(|_| Msg::Noop);
            self.view_face_card(card, selected, false, on_click, on_double_click)
        } else {
            html! {
                <button
                    type="button"
                    class={classes!("pile-empty", selected.then_some("selected"))}
                    onclick={on_click}
                    aria-label={format!("Foundation {}", pile + 1)}
                    disabled={self.interactions_locked()}
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
        Self {
            game: GameState::new_shuffled(),
            status: "Draw from stock and build the four temples from Ace to King.".to_string(),
            end_state: None,
            help_expanded: false,
            victory_gold_award: 0,
            victory_rain_dismissed: false,
            all_to_temple_running: false,
            all_to_temple_timeout: None,
            key_listener: None,
        }
    }

    fn rendered(&mut self, ctx: &Context<Self>, first_render: bool) {
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
        if self.end_state.is_some()
            && !matches!(
                msg,
                Msg::Noop | Msg::NewGame | Msg::ToggleHelp | Msg::DismissVictoryRain
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
                let draw_count = self.game.draw_count;
                self.game = GameState::new_shuffled_with_draw_count(draw_count);
                self.stop_all_to_temple();
                self.end_state = None;
                self.victory_gold_award = 0;
                self.victory_rain_dismissed = false;
                self.status = "You gave up. A fresh deck has been dealt.".to_string();
            }
            Msg::DrawStock => {
                let had_stock = !self.game.stock.is_empty();
                let had_waste = !self.game.waste.is_empty();
                let waste_before = self.game.waste.len();
                let gold_before = self.game.temple_gold;
                self.game.draw_or_recycle();
                self.status = if had_stock {
                    let drawn = self.game.waste.len().saturating_sub(waste_before);
                    let suffix = if drawn == 1 { "" } else { "s" };
                    format!("Drew {drawn} card{suffix} to the waste pile.")
                } else if had_waste {
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
                if self.game.waste.is_empty() {
                    self.status = "Waste pile is empty.".to_string();
                } else {
                    let _ = self.game.select_waste();
                    if self.game.move_selected_to_any_foundation() {
                        self.status = "Moved waste card to a foundation.".to_string();
                    } else {
                        self.game.clear_selection();
                        self.status = "Waste card cannot move to any foundation yet.".to_string();
                    }
                }
            }
            Msg::ClickFoundation(pile) => {
                if self.game.selected.is_some() {
                    if self.game.move_selected_to_foundation(pile) {
                        self.status = format!("Placed card on foundation {}.", pile + 1);
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
                        self.status = "Illegal foundation move.".to_string();
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
                if self.game.selected.is_some() {
                    if self.game.move_selected_to_tableau(pile) {
                        self.status = format!("Moved cards to tableau column {}.", pile + 1);
                    } else if self.game.select_tableau(pile, index) {
                        if let Some(card) = self.game.selected_card() {
                            self.status = format!(
                                "Selected tableau run starting at {}.",
                                Self::describe_card(card)
                            );
                        }
                    } else {
                        self.status = "Illegal tableau move.".to_string();
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
                if self.game.select_tableau(pile, index) {
                    if self.game.move_selected_to_any_foundation() {
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
                if self.game.selected.is_some() {
                    if self.game.move_selected_to_tableau(pile) {
                        self.status = format!("Moved cards to tableau column {}.", pile + 1);
                    } else {
                        self.status =
                            "Only a King can move into an empty tableau column.".to_string();
                    }
                } else if let Some(top_index) = self.game.tableau[pile].len().checked_sub(1)
                    && self.game.tableau[pile][top_index].face_up
                    && self.game.select_tableau(pile, top_index)
                    && let Some(card) = self.game.selected_card()
                {
                    self.status = format!("Selected top card {}.", Self::describe_card(card));
                }
            }
            Msg::AutoFoundation => {
                if self.game.auto_promote_lowest() {
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
                if self.game.auto_promote_lowest() {
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

                if self.game.auto_promote_lowest() {
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
                self.end_state = Some(EndState::ZeusThunder);
                self.status = "Zeus' Thunder is heard".to_string();
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
        }

        if self.game.won && self.end_state.is_none() {
            self.trigger_victory();
        }
        if self.end_state.is_none() && !self.game.has_any_legal_move() {
            self.stop_all_to_temple();
            self.game.zeus_vision();
            self.end_state = Some(EndState::Stalemate);
            self.status = "Zeus demands obeisance! Only surrender remains.".to_string();
        }

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

        let stock_view = if self.game.stock.is_empty() {
            let label = if self.game.waste.is_empty() {
                "Stock"
            } else {
                "Recycle waste"
            };
            html! {
                <button type="button" class="pile-empty stock-empty" onclick={draw_stock.clone()} aria-label={label} disabled={locked}>
                    <span>{ "REDEAL" }</span>
                    <span class="tiny">{ "STOCK" }</span>
                </button>
            }
        } else {
            self.view_back_card(false, "Draw from stock", draw_stock.clone())
        };

        let waste_view = if let Some(card) = self.game.waste.last().copied() {
            let selected = self.game.is_selected(Selection::Waste);
            self.view_face_card(card, selected, false, click_waste, double_click_waste)
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
                        { self.view_foundation_slot(ctx, pile) }
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
                let pile_click = ctx.link().callback(move |_| Msg::ClickTableauPile(pile_index));
                let mut offset = 0usize;
                let mut visible_height = 0usize;

                let cards = pile
                    .iter()
                    .enumerate()
                    .map(|(card_index, tableau_card)| {
                        let top = offset;
                        offset += if tableau_card.face_up { 30 } else { 14 };
                        visible_height = visible_height.max(top + 150);

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
                            self.view_face_card(
                                tableau_card.card,
                                selected,
                                tableau_card.zeus_revealed,
                                on_click,
                                on_double_click,
                            )
                        } else {
                            let block_click = ctx.link().callback(|event: MouseEvent| {
                                event.stop_propagation();
                                Msg::Noop
                            });
                            self.view_back_card(false, "Hidden card", block_click)
                        };

                        html! {
                            <div class="tableau-layer" style={format!("top: {top}px;")}> { card_html } </div>
                        }
                    })
                    .collect::<Html>();

                let mut pile_classes = classes!("tableau-pile");
                if pile.is_empty() {
                    pile_classes.push("empty");
                    visible_height = 150;
                }

                html! {
                    <div class="tableau-column">
                        <div class="pile-label">{ format!("Column {}", pile_index + 1) }</div>
                        <div
                            class={pile_classes}
                            onclick={
                                if locked {
                                    ctx.link().callback(|_| Msg::Noop)
                                } else {
                                    pile_click
                                }
                            }
                            style={format!("height: {}px;", visible_height.max(150))}
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
                    <div class="draw-group">
                        <div class="pile-slot">
                            <div class="pile-label">{ "Stock" }</div>
                            { stock_view }
                        </div>
                        <div class="pile-slot">
                            <div class="pile-label">{ "Waste" }</div>
                            { waste_view }
                        </div>
                    </div>
                    <div class="foundation-group">
                        { foundation_slots }
                    </div>
                </section>

                <section class="tableau-scroll">
                    <div class="tableau-grid">
                        { tableau_columns }
                    </div>
                </section>

                <section class={classes!("help-strip", (!self.help_expanded).then_some("collapsed"))}>
                    <span>{ "Click to select and move." }</span>
                    <span>{ "Double-click waste/top tableau card to send it to a temple." }</span>
                    <span>{ "Build tableau in descending alternating colors." }</span>
                    <span>{ "Zeus' Vision reveals hidden cards and ends the game." }</span>
                    <span>{ "All To Temple auto-runs endgame moves until no temple move remains." }</span>
                    <span>{ "Keys: D or Enter draws, Space sends one to temple, A sends all." }</span>
                </section>
                <span class="version-tag" aria-hidden="true">{ concat!("v", env!("CARGO_PKG_VERSION")) }</span>
            </main>
        }
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    info!("Starting Solitare of Olympus");
    Renderer::<App>::new().render();
}
