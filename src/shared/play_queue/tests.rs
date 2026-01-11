use super::*;

fn create_test_song(title: &str) -> Song {
    let mut song = Song::default();
    song.uri = format!("video_{}", title);
    song.metadata.insert("title".into(), vec![title.to_string()]);
    song
}

#[test]
fn test_add_single_song() {
    let mut queue = PlayQueue::new();
    let events = queue.apply(QueueCommand::Add { song: create_test_song("test") });

    assert_eq!(queue.len(), 1);
    assert_eq!(events.len(), 1);
    match &events[0] {
        QueueEvent::ItemsAdded { ids } => assert_eq!(ids.len(), 1),
        _ => panic!("Expected ItemsAdded event"),
    }
}

#[test]
fn test_add_batch() {
    let mut queue = PlayQueue::new();
    let songs = vec![create_test_song("one"), create_test_song("two"), create_test_song("three")];

    let events = queue.apply(QueueCommand::AddBatch { songs });

    assert_eq!(queue.len(), 3);
    match &events[0] {
        QueueEvent::ItemsAdded { ids } => assert_eq!(ids.len(), 3),
        _ => panic!("Expected ItemsAdded event"),
    }
}

#[test]
fn test_clear_queue() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    let events = queue.apply(QueueCommand::Clear);

    assert_eq!(queue.len(), 0);
    assert!(matches!(events[0], QueueEvent::Cleared));
}

#[test]
fn test_play_sets_current() {
    let mut queue = PlayQueue::new();
    let events = queue.apply(QueueCommand::Add { song: create_test_song("test") });

    let id = match &events[0] {
        QueueEvent::ItemsAdded { ids } => ids[0],
        _ => panic!("Expected ItemsAdded"),
    };

    let play_events = queue.apply(QueueCommand::Play { id });

    assert_eq!(queue.get_current_id(), Some(id));
    match &play_events[0] {
        QueueEvent::CurrentChanged { from, to } => {
            assert_eq!(*from, None);
            assert_eq!(*to, Some(id));
        }
        _ => panic!("Expected CurrentChanged event"),
    }
}

#[test]
fn test_next_advances_to_next_song() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two")],
    });

    queue.apply(QueueCommand::Play { id: 1 });
    let events = queue.apply(QueueCommand::Next);

    assert_eq!(queue.get_current_id(), Some(2));
    match &events[0] {
        QueueEvent::CurrentChanged { from, to } => {
            assert_eq!(*from, Some(1));
            assert_eq!(*to, Some(2));
        }
        _ => panic!("Expected CurrentChanged event"),
    }
}

#[test]
fn test_next_at_end_without_repeat_returns_none() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("only") });

    queue.apply(QueueCommand::Play { id: 1 });
    queue.apply(QueueCommand::Next);

    assert_eq!(queue.get_current_id(), None);
}

#[test]
fn test_next_at_end_with_repeat_all_wraps() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two")],
    });

    queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::All });
    queue.apply(QueueCommand::Play { id: 2 });
    queue.apply(QueueCommand::Next);

    assert_eq!(queue.get_current_id(), Some(1));
}

#[test]
fn test_advance_with_repeat_one_stays_on_current() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::One });
    queue.apply(QueueCommand::Play { id: 1 });

    let events = queue.apply(QueueCommand::Advance);

    assert_eq!(queue.get_current_id(), Some(1));
    assert_eq!(events.len(), 0);
}

#[test]
fn test_previous_in_sequential_mode() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two")],
    });

    queue.apply(QueueCommand::Play { id: 2 });
    queue.apply(QueueCommand::Previous);

    assert_eq!(queue.get_current_id(), Some(1));
}

#[test]
fn test_previous_from_history_in_shuffle_mode() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B"), create_test_song("C")],
    });
    queue.apply(QueueCommand::Play { id: 1 });
    queue.apply(QueueCommand::Next);
    queue.apply(QueueCommand::Next);
    queue.apply(QueueCommand::SetShuffle { enabled: true });

    queue.apply(QueueCommand::Previous);

    assert_eq!(queue.get_current_id(), Some(2));
}

#[test]
fn test_shuffle_changes_order() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two"), create_test_song("three")],
    });

    let original = queue.get_play_order().to_vec();

    queue.apply(QueueCommand::SetShuffle { enabled: true });

    let shuffled = queue.get_play_order().to_vec();

    assert_eq!(original.len(), shuffled.len());
    assert_eq!(queue.get_original_order().to_vec(), original);
}

#[test]
fn test_shuffle_keeps_current_first() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two"), create_test_song("three")],
    });

    queue.apply(QueueCommand::Play { id: 2 });
    queue.apply(QueueCommand::SetShuffle { enabled: true });

    assert_eq!(queue.get_play_order()[0], 2);
}

#[test]
fn test_shuffle_emits_order_changed_event() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B")],
    });
    queue.apply(QueueCommand::Play { id: 1 });

    let events = queue.apply(QueueCommand::SetShuffle { enabled: true });

    assert!(events.iter().any(|e| matches!(e, QueueEvent::OrderChanged { .. })));
    assert!(events.iter().any(|e| matches!(e, QueueEvent::ModesChanged { shuffle: true, .. })));
}

#[test]
fn test_unshuffle_restores_original_order() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two"), create_test_song("three")],
    });

    let original = queue.get_play_order().to_vec();

    queue.apply(QueueCommand::SetShuffle { enabled: true });
    queue.apply(QueueCommand::SetShuffle { enabled: false });

    assert_eq!(queue.get_play_order(), &original[..]);
}

#[test]
fn test_remove_current_song_stops() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    queue.apply(QueueCommand::Play { id: 1 });
    let events = queue.apply(QueueCommand::Remove { id: 1 });

    assert_eq!(queue.get_current_id(), None);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], QueueEvent::ItemsRemoved { .. }));
    assert!(matches!(events[1], QueueEvent::CurrentChanged { .. }));
}

#[test]
fn test_remove_updates_both_orders() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B"), create_test_song("C")],
    });

    let events = queue.apply(QueueCommand::Remove { id: 2 });

    assert_eq!(queue.len(), 2);
    assert!(!queue.get_play_order().contains(&2));
    assert!(!queue.get_original_order().contains(&2));
    assert!(events.iter().any(|e| matches!(e, QueueEvent::ItemsRemoved { .. })));
}

#[test]
fn test_remove_nonexistent_is_noop() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    let events = queue.apply(QueueCommand::Remove { id: 999 });

    assert_eq!(queue.len(), 1);
    assert!(events.is_empty());
}

#[test]
fn test_stop_command() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    queue.apply(QueueCommand::Play { id: 1 });
    let events = queue.apply(QueueCommand::Stop);

    assert_eq!(queue.get_current_id(), None);
    assert!(events.iter().any(|e| matches!(e, QueueEvent::Stopped)));
}

#[test]
fn test_stop_when_not_playing() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    let events = queue.apply(QueueCommand::Stop);

    assert!(events.iter().any(|e| matches!(e, QueueEvent::Stopped)));
    assert_eq!(events.len(), 1);
}

#[test]
fn test_move_song() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two"), create_test_song("three")],
    });

    queue.apply(QueueCommand::Move { id: 1, to_position: 2 });

    let order = queue.get_play_order();
    assert_eq!(order[2], 1);
}

#[test]
fn test_move_emits_order_changed() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B")],
    });

    let events = queue.apply(QueueCommand::Move { id: 1, to_position: 1 });

    assert!(events.iter().any(|e| matches!(e, QueueEvent::OrderChanged { .. })));
}

#[test]
fn test_move_to_same_position_is_noop() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B")],
    });

    let events = queue.apply(QueueCommand::Move { id: 1, to_position: 0 });

    assert!(events.is_empty());
}

#[test]
fn test_move_nonexistent_is_noop() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("A") });

    let events = queue.apply(QueueCommand::Move { id: 999, to_position: 0 });

    assert!(events.is_empty());
}

#[test]
fn test_add_during_shuffle_appends_to_end() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two")],
    });

    queue.apply(QueueCommand::SetShuffle { enabled: true });

    queue.apply(QueueCommand::Add { song: create_test_song("new") });

    let order = queue.get_play_order();
    assert_eq!(order[order.len() - 1], 3);
}

#[test]
fn test_get_position() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two"), create_test_song("three")],
    });

    assert_eq!(queue.get_position(1), Some(0));
    assert_eq!(queue.get_position(2), Some(1));
    assert_eq!(queue.get_position(3), Some(2));
    assert_eq!(queue.get_position(999), None);
}

#[test]
fn test_get_current_position() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two")],
    });

    queue.apply(QueueCommand::Play { id: 2 });

    assert_eq!(queue.get_current_position(), Some(1));
}

#[test]
fn test_get_current_position_when_not_playing() {
    let queue = PlayQueue::new();
    assert_eq!(queue.get_current_position(), None);
}

#[test]
fn test_repeat_mode_events() {
    let mut queue = PlayQueue::new();

    let events = queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::All });

    assert_eq!(queue.get_repeat(), RepeatMode::All);
    match &events[0] {
        QueueEvent::ModesChanged { shuffle, repeat } => {
            assert!(!*shuffle);
            assert_eq!(*repeat, RepeatMode::All);
        }
        _ => panic!("Expected ModesChanged event"),
    }
}

#[test]
fn test_no_event_when_setting_same_mode() {
    let mut queue = PlayQueue::new();

    queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::All });

    let events = queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::All });

    assert_eq!(events.len(), 0);
}

#[test]
fn test_no_event_when_setting_same_shuffle() {
    let mut queue = PlayQueue::new();

    let events = queue.apply(QueueCommand::SetShuffle { enabled: false });

    assert!(events.is_empty());
}

#[test]
fn test_is_empty() {
    let mut queue = PlayQueue::new();
    assert!(queue.is_empty());

    queue.apply(QueueCommand::Add { song: create_test_song("test") });
    assert!(!queue.is_empty());

    queue.apply(QueueCommand::Clear);
    assert!(queue.is_empty());
}

#[test]
fn test_get_song() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    let song = queue.get_song(1);
    assert!(song.is_some());
    assert_eq!(song.unwrap().uri, "video_test");

    assert!(queue.get_song(999).is_none());
}

#[test]
fn test_get_songs_in_order() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B"), create_test_song("C")],
    });

    let songs = queue.get_songs_in_order();
    assert_eq!(songs.len(), 3);
    assert_eq!(songs[0].uri, "video_A");
    assert_eq!(songs[1].uri, "video_B");
    assert_eq!(songs[2].uri, "video_C");
}

#[test]
fn test_get_shuffle() {
    let mut queue = PlayQueue::new();
    assert!(!queue.get_shuffle());

    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B")],
    });

    queue.apply(QueueCommand::SetShuffle { enabled: true });
    assert!(queue.get_shuffle());

    queue.apply(QueueCommand::SetShuffle { enabled: false });
    assert!(!queue.get_shuffle());
}

#[test]
fn test_default_trait() {
    let queue = PlayQueue::default();
    assert!(queue.is_empty());
    assert!(!queue.get_shuffle());
    assert_eq!(queue.get_repeat(), RepeatMode::Off);
}

#[test]
fn test_previous_with_repeat_all_wraps() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("one"), create_test_song("two")],
    });

    queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::All });
    queue.apply(QueueCommand::Play { id: 1 });
    queue.apply(QueueCommand::Previous);

    assert_eq!(queue.get_current_id(), Some(2));
}

#[test]
fn test_play_nonexistent_is_noop() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::Add { song: create_test_song("test") });

    let events = queue.apply(QueueCommand::Play { id: 999 });

    assert!(events.is_empty());
    assert_eq!(queue.get_current_id(), None);
}

#[test]
fn test_play_adds_previous_to_history() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B"), create_test_song("C")],
    });

    queue.apply(QueueCommand::Play { id: 1 });
    queue.apply(QueueCommand::Play { id: 2 });
    queue.apply(QueueCommand::Play { id: 3 });
    queue.apply(QueueCommand::SetShuffle { enabled: true });
    queue.apply(QueueCommand::Previous);

    assert_eq!(queue.get_current_id(), Some(2));
}

#[test]
fn test_clear_emits_cleared_event() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B")],
    });
    queue.apply(QueueCommand::Play { id: 1 });

    let events = queue.apply(QueueCommand::Clear);

    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], QueueEvent::Cleared));
    assert_eq!(queue.get_current_id(), None);
}

#[test]
fn test_advance_respects_repeat_all() {
    let mut queue = PlayQueue::new();
    queue.apply(QueueCommand::AddBatch {
        songs: vec![create_test_song("A"), create_test_song("B")],
    });
    queue.apply(QueueCommand::SetRepeat { mode: RepeatMode::All });
    queue.apply(QueueCommand::Play { id: 2 });

    queue.apply(QueueCommand::Advance);

    assert_eq!(queue.get_current_id(), Some(1));
}
