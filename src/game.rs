use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha20Rng;

pub const HARD_DRAW_COUNT: usize = 3;
pub const EASY_DRAW_COUNT: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suit {
    Clubs,
    Diamonds,
    Hearts,
    Spades,
}

impl Suit {
    pub fn is_red(self) -> bool {
        matches!(self, Self::Diamonds | Self::Hearts)
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Clubs => "♣",
            Self::Diamonds => "♦",
            Self::Hearts => "♥",
            Self::Spades => "♠",
        }
    }

    pub fn latin_name(self) -> &'static str {
        match self {
            Self::Clubs => "LAUREL",
            Self::Diamonds => "GOLD",
            Self::Hearts => "CUPID",
            Self::Spades => "BACCHUS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    pub suit: Suit,
    pub rank: u8,
}

impl Card {
    pub fn rank_label(self) -> &'static str {
        match self.rank {
            1 => "A",
            2 => "2",
            3 => "3",
            4 => "4",
            5 => "5",
            6 => "6",
            7 => "7",
            8 => "8",
            9 => "9",
            10 => "10",
            11 => "J",
            12 => "Q",
            13 => "K",
            _ => "?",
        }
    }

    pub fn is_red(self) -> bool {
        self.suit.is_red()
    }

    pub fn motif(self) -> &'static str {
        match self.rank {
            1 => "LAUREL",
            11 => "CUPID",
            12 => "IVY",
            13 => "BACCHUS",
            _ => self.suit.latin_name(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableauCard {
    pub card: Card,
    pub face_up: bool,
    pub zeus_revealed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Waste,
    Foundation { pile: usize },
    Tableau { pile: usize, index: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameState {
    pub stock: Vec<Card>,
    pub waste: Vec<Card>,
    pub foundations: [Vec<Card>; 4],
    pub tableau: [Vec<TableauCard>; 7],
    pub draw_count: usize,
    pub selected: Option<Selection>,
    pub moves: usize,
    pub temple_gold: usize,
    pub won: bool,
}

impl Default for GameState {
    fn default() -> Self {
        Self::new_shuffled()
    }
}

impl GameState {
    pub fn new_shuffled() -> Self {
        Self::new_shuffled_with_draw_count(HARD_DRAW_COUNT)
    }

    pub fn new_shuffled_with_draw_count(draw_count: usize) -> Self {
        let draw_count = sanitize_draw_count(draw_count);
        let mut deck = full_deck();
        shuffle_deck(&mut deck);

        let mut tableau: [Vec<TableauCard>; 7] = std::array::from_fn(|_| Vec::new());
        for (pile, pile_cards) in tableau.iter_mut().enumerate() {
            for row in 0..=pile {
                let card = deck.pop().expect("deck should have enough cards for deal");
                pile_cards.push(TableauCard {
                    card,
                    face_up: row == pile,
                    zeus_revealed: false,
                });
            }
        }

        Self {
            stock: deck,
            waste: Vec::new(),
            foundations: std::array::from_fn(|_| Vec::new()),
            tableau,
            draw_count,
            selected: None,
            moves: 0,
            temple_gold: 0,
            won: false,
        }
    }

    pub fn empty() -> Self {
        Self {
            stock: Vec::new(),
            waste: Vec::new(),
            foundations: std::array::from_fn(|_| Vec::new()),
            tableau: std::array::from_fn(|_| Vec::new()),
            draw_count: HARD_DRAW_COUNT,
            selected: None,
            moves: 0,
            temple_gold: 0,
            won: false,
        }
    }

    pub fn set_draw_count(&mut self, draw_count: usize) {
        self.draw_count = sanitize_draw_count(draw_count);
    }

    pub fn draw_or_recycle(&mut self) {
        self.selected = None;

        let mut drew_any = false;
        for _ in 0..self.draw_count {
            let Some(card) = self.stock.pop() else {
                break;
            };
            drew_any = true;
            self.waste.push(card);
        }

        if drew_any {
            self.moves += 1;
            return;
        }

        if !self.waste.is_empty() {
            self.stock = self.waste.drain(..).rev().collect();
            self.moves += 1;
            self.temple_gold = self.temple_gold.saturating_sub(1);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn zeus_vision(&mut self) -> usize {
        self.selected = None;
        let mut revealed = 0;

        for pile in &mut self.tableau {
            for tableau_card in pile {
                if !tableau_card.face_up {
                    tableau_card.face_up = true;
                    tableau_card.zeus_revealed = true;
                    revealed += 1;
                }
            }
        }

        revealed
    }

    pub fn is_selected(&self, selection: Selection) -> bool {
        self.selected == Some(selection)
    }

    pub fn select_waste(&mut self) -> bool {
        if self.waste.is_empty() {
            return false;
        }

        let selection = Selection::Waste;
        if self.selected == Some(selection) {
            self.selected = None;
        } else {
            self.selected = Some(selection);
        }
        true
    }

    pub fn select_foundation(&mut self, pile: usize) -> bool {
        if pile >= self.foundations.len() || self.foundations[pile].is_empty() {
            return false;
        }

        let selection = Selection::Foundation { pile };
        if self.selected == Some(selection) {
            self.selected = None;
        } else {
            self.selected = Some(selection);
        }
        true
    }

    pub fn can_select_tableau(&self, pile: usize, index: usize) -> bool {
        if pile >= self.tableau.len() {
            return false;
        }

        let cards = &self.tableau[pile];
        if index >= cards.len() || !cards[index].face_up {
            return false;
        }

        for i in index..cards.len().saturating_sub(1) {
            let lower = cards[i].card;
            let upper = cards[i + 1].card;
            if !cards[i + 1].face_up {
                return false;
            }
            if lower.is_red() == upper.is_red() {
                return false;
            }
            if lower.rank != upper.rank + 1 {
                return false;
            }
        }

        true
    }

    pub fn select_tableau(&mut self, pile: usize, index: usize) -> bool {
        if !self.can_select_tableau(pile, index) {
            return false;
        }

        let selection = Selection::Tableau { pile, index };
        if self.selected == Some(selection) {
            self.selected = None;
        } else {
            self.selected = Some(selection);
        }
        true
    }

    pub fn move_selected_to_any_foundation(&mut self) -> bool {
        for foundation in 0..self.foundations.len() {
            if self.move_selected_to_foundation(foundation) {
                return true;
            }
        }
        false
    }

    pub fn move_selected_to_foundation(&mut self, target: usize) -> bool {
        if target >= self.foundations.len() {
            return false;
        }

        let selection = match self.selected {
            Some(selection) => selection,
            None => return false,
        };

        let card = match selection {
            Selection::Waste => match self.waste.last().copied() {
                Some(card) => card,
                None => return false,
            },
            Selection::Foundation { .. } => {
                // Foundation-to-foundation moves are intentionally disallowed.
                return false;
            }
            Selection::Tableau { pile, index } => {
                if pile >= self.tableau.len() {
                    return false;
                }

                let cards = &self.tableau[pile];
                if index + 1 != cards.len() {
                    // Only top cards can move to foundations.
                    return false;
                }

                match cards.get(index).copied() {
                    Some(tableau_card) if tableau_card.face_up => tableau_card.card,
                    _ => return false,
                }
            }
        };

        if !can_place_on_foundation(card, &self.foundations[target]) {
            return false;
        }

        match selection {
            Selection::Waste => {
                self.waste.pop();
            }
            Selection::Foundation { .. } => {
                return false;
            }
            Selection::Tableau { pile, .. } => {
                self.tableau[pile].pop();
                self.flip_tableau_top(pile);
            }
        }

        self.foundations[target].push(card);
        self.selected = None;
        self.moves += 1;
        self.temple_gold += 1;
        self.refresh_win();
        true
    }

    pub fn move_selected_to_tableau(&mut self, target: usize) -> bool {
        if target >= self.tableau.len() {
            return false;
        }

        let selection = match self.selected {
            Some(selection) => selection,
            None => return false,
        };

        let first_card = match selection {
            Selection::Waste => match self.waste.last().copied() {
                Some(card) => card,
                None => return false,
            },
            Selection::Foundation { pile } => {
                match self.foundations.get(pile).and_then(|p| p.last()).copied() {
                    Some(card) => card,
                    None => return false,
                }
            }
            Selection::Tableau { pile, index } => {
                if pile == target || !self.can_select_tableau(pile, index) {
                    return false;
                }

                match self.tableau[pile].get(index).copied() {
                    Some(card) => card.card,
                    None => return false,
                }
            }
        };

        if !can_place_on_tableau(first_card, &self.tableau[target]) {
            return false;
        }

        let gold_earned = match selection {
            Selection::Waste => {
                let card = match self.waste.pop() {
                    Some(card) => card,
                    None => return false,
                };
                self.tableau[target].push(TableauCard {
                    card,
                    face_up: true,
                    zeus_revealed: false,
                });
                1
            }
            Selection::Foundation { pile } => {
                let card = match self.foundations[pile].pop() {
                    Some(card) => card,
                    None => return false,
                };
                self.tableau[target].push(TableauCard {
                    card,
                    face_up: true,
                    zeus_revealed: false,
                });
                1
            }
            Selection::Tableau { pile, index } => {
                let moved: Vec<TableauCard> = self.tableau[pile].drain(index..).collect();
                self.tableau[target].extend(moved);
                self.flip_tableau_top(pile);
                0
            }
        };

        self.selected = None;
        self.moves += 1;
        self.temple_gold += gold_earned;
        self.refresh_win();
        true
    }

    pub fn selected_card(&self) -> Option<Card> {
        match self.selected {
            Some(Selection::Waste) => self.waste.last().copied(),
            Some(Selection::Foundation { pile }) => {
                self.foundations.get(pile).and_then(|p| p.last()).copied()
            }
            Some(Selection::Tableau { pile, index }) => self
                .tableau
                .get(pile)
                .and_then(|p| p.get(index).copied())
                .filter(|c| c.face_up)
                .map(|c| c.card),
            None => None,
        }
    }

    fn flip_tableau_top(&mut self, pile: usize) {
        if let Some(top) = self.tableau[pile].last_mut()
            && !top.face_up
        {
            top.face_up = true;
            top.zeus_revealed = false;
        }
    }

    fn refresh_win(&mut self) {
        self.won = self.foundations.iter().all(|pile| pile.len() == 13);
    }
}

fn sanitize_draw_count(draw_count: usize) -> usize {
    if draw_count <= EASY_DRAW_COUNT {
        EASY_DRAW_COUNT
    } else {
        HARD_DRAW_COUNT
    }
}

fn can_place_on_foundation(card: Card, pile: &[Card]) -> bool {
    match pile.last().copied() {
        Some(top) => top.suit == card.suit && card.rank == top.rank + 1,
        None => card.rank == 1,
    }
}

fn can_place_on_tableau(card: Card, pile: &[TableauCard]) -> bool {
    match pile.last().copied() {
        Some(top) => {
            top.face_up && top.card.is_red() != card.is_red() && card.rank + 1 == top.card.rank
        }
        None => card.rank == 13,
    }
}

fn full_deck() -> Vec<Card> {
    let mut deck = Vec::with_capacity(52);
    let suits = [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades];

    for suit in suits {
        for rank in 1..=13 {
            deck.push(Card { suit, rank });
        }
    }

    deck
}

fn shuffle_deck(deck: &mut [Card]) {
    let seed = secure_seed();
    let mut rng = ChaCha20Rng::from_seed(seed);
    deck.shuffle(&mut rng);
}

fn secure_seed() -> [u8; 32] {
    let mut seed = [0_u8; 32];
    if getrandom::fill(&mut seed).is_ok() {
        return seed;
    }

    fallback_seed()
}

#[cfg(target_arch = "wasm32")]
fn fallback_seed() -> [u8; 32] {
    let mut seed = [0_u8; 32];
    let nanos = (js_sys::Date::now() * 1_000_000.0) as u64;
    for (idx, byte) in seed.iter_mut().enumerate() {
        let rotated = nanos.rotate_left((idx % 64) as u32);
        *byte = (rotated as u8) ^ (idx as u8).wrapping_mul(37);
    }
    seed
}

#[cfg(not(target_arch = "wasm32"))]
fn fallback_seed() -> [u8; 32] {
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut seed = [0_u8; 32];
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as u64 ^ 0x9E37_79B9_7F4A_7C15,
        Err(_) => 0xD1B5_4A32_CAF0_0042,
    };

    for (idx, byte) in seed.iter_mut().enumerate() {
        let rotated = nanos.rotate_left((idx % 64) as u32);
        *byte = (rotated as u8) ^ (idx as u8).wrapping_mul(53);
    }

    seed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(rank: u8, suit: Suit) -> Card {
        Card { rank, suit }
    }

    #[test]
    fn initial_deal_has_correct_counts() {
        let game = GameState::new_shuffled();

        let tableau_total: usize = game.tableau.iter().map(Vec::len).sum();
        assert_eq!(tableau_total, 28);
        assert_eq!(game.stock.len(), 24);
        assert!(game.waste.is_empty());
        assert!(game.foundations.iter().all(Vec::is_empty));
        assert_eq!(tableau_total + game.stock.len(), 52);

        for (idx, pile) in game.tableau.iter().enumerate() {
            assert_eq!(pile.len(), idx + 1);
            assert!(pile.last().is_some_and(|top| top.face_up));
        }
    }

    #[test]
    fn king_can_move_to_empty_tableau() {
        let mut game = GameState::empty();
        game.waste.push(c(13, Suit::Spades));

        assert!(game.select_waste());
        assert!(game.move_selected_to_tableau(0));

        assert!(game.waste.is_empty());
        assert_eq!(game.tableau[0].len(), 1);
        assert_eq!(game.tableau[0][0].card.rank, 13);
        assert!(game.selected.is_none());
    }

    #[test]
    fn non_king_cannot_move_to_empty_tableau() {
        let mut game = GameState::empty();
        game.waste.push(c(10, Suit::Hearts));

        assert!(game.select_waste());
        assert!(!game.move_selected_to_tableau(0));

        assert_eq!(game.waste.len(), 1);
        assert!(game.tableau[0].is_empty());
    }

    #[test]
    fn moving_tableau_run_flips_new_top() {
        let mut game = GameState::empty();
        game.tableau[0] = vec![
            TableauCard {
                card: c(9, Suit::Hearts),
                face_up: false,
                zeus_revealed: false,
            },
            TableauCard {
                card: c(8, Suit::Clubs),
                face_up: true,
                zeus_revealed: false,
            },
        ];
        game.tableau[1] = vec![TableauCard {
            card: c(9, Suit::Diamonds),
            face_up: true,
            zeus_revealed: false,
        }];

        assert!(game.select_tableau(0, 1));
        assert!(game.move_selected_to_tableau(1));

        assert_eq!(game.tableau[0].len(), 1);
        assert!(game.tableau[0][0].face_up);
        assert_eq!(game.tableau[1].len(), 2);
        assert_eq!(game.tableau[1][1].card.rank, 8);
    }

    #[test]
    fn foundation_builds_by_suit() {
        let mut game = GameState::empty();

        game.waste.push(c(1, Suit::Hearts));
        assert!(game.select_waste());
        assert!(game.move_selected_to_foundation(0));

        game.waste.push(c(2, Suit::Hearts));
        assert!(game.select_waste());
        assert!(game.move_selected_to_foundation(0));

        game.waste.push(c(3, Suit::Spades));
        assert!(game.select_waste());
        assert!(!game.move_selected_to_foundation(0));

        assert_eq!(game.foundations[0].len(), 2);
        assert_eq!(game.foundations[0][0].rank, 1);
        assert_eq!(game.foundations[0][1].rank, 2);
    }

    #[test]
    fn only_top_tableau_card_can_go_to_foundation() {
        let mut game = GameState::empty();
        game.foundations[0].push(c(1, Suit::Hearts));
        game.tableau[0] = vec![
            TableauCard {
                card: c(2, Suit::Hearts),
                face_up: true,
                zeus_revealed: false,
            },
            TableauCard {
                card: c(1, Suit::Clubs),
                face_up: true,
                zeus_revealed: false,
            },
        ];

        assert!(game.select_tableau(0, 0));
        assert!(!game.move_selected_to_foundation(0));

        assert!(game.select_tableau(0, 1));
        assert!(game.move_selected_to_foundation(1));
        assert_eq!(game.foundations[0].len(), 1);
        assert_eq!(game.foundations[1].len(), 1);
        assert_eq!(game.temple_gold, 1);
    }

    #[test]
    fn tableau_to_tableau_move_awards_no_gold() {
        let mut game = GameState::empty();
        game.tableau[0] = vec![
            TableauCard {
                card: c(8, Suit::Clubs),
                face_up: true,
                zeus_revealed: false,
            },
            TableauCard {
                card: c(7, Suit::Hearts),
                face_up: true,
                zeus_revealed: false,
            },
        ];
        game.tableau[1] = vec![TableauCard {
            card: c(9, Suit::Diamonds),
            face_up: true,
            zeus_revealed: false,
        }];

        assert!(game.select_tableau(0, 0));
        assert!(game.move_selected_to_tableau(1));
        assert_eq!(game.temple_gold, 0);
    }

    #[test]
    fn temple_gold_decreases_on_recycle() {
        let mut game = GameState::empty();
        game.temple_gold = 2;
        game.waste.push(c(4, Suit::Diamonds));
        game.waste.push(c(8, Suit::Spades));

        game.draw_or_recycle();

        assert_eq!(game.temple_gold, 1);
        assert_eq!(game.stock.len(), 2);
        assert!(game.waste.is_empty());
    }

    #[test]
    fn hard_mode_draws_three_cards_from_stock() {
        let mut game = GameState::empty();
        game.stock = vec![
            c(1, Suit::Clubs),
            c(2, Suit::Clubs),
            c(3, Suit::Clubs),
            c(4, Suit::Clubs),
        ];

        game.draw_or_recycle();

        assert_eq!(game.draw_count, HARD_DRAW_COUNT);
        assert_eq!(game.stock.len(), 1);
        assert_eq!(game.waste.len(), 3);
        assert_eq!(game.moves, 1);
    }

    #[test]
    fn easy_mode_draws_one_card_from_stock() {
        let mut game = GameState::empty();
        game.set_draw_count(EASY_DRAW_COUNT);
        game.stock = vec![c(1, Suit::Spades), c(2, Suit::Spades), c(3, Suit::Spades)];

        game.draw_or_recycle();

        assert_eq!(game.draw_count, EASY_DRAW_COUNT);
        assert_eq!(game.stock.len(), 2);
        assert_eq!(game.waste.len(), 1);
        assert_eq!(game.moves, 1);
    }

    #[test]
    fn zeus_vision_reveals_all_hidden_tableau_cards() {
        let mut game = GameState::empty();
        game.tableau[0] = vec![
            TableauCard {
                card: c(4, Suit::Clubs),
                face_up: false,
                zeus_revealed: false,
            },
            TableauCard {
                card: c(3, Suit::Hearts),
                face_up: false,
                zeus_revealed: false,
            },
            TableauCard {
                card: c(2, Suit::Spades),
                face_up: true,
                zeus_revealed: false,
            },
        ];

        let revealed = game.zeus_vision();

        assert_eq!(revealed, 2);
        assert!(game.tableau[0].iter().all(|card| card.face_up));
        assert!(game.tableau[0][0].zeus_revealed);
        assert!(game.tableau[0][1].zeus_revealed);
        assert!(!game.tableau[0][2].zeus_revealed);
    }
}
