//! Tests for queue service

#[cfg(test)]
mod tests {
    use super::super::QueueService;
    use crate::domain::Song;

    fn create_test_song(title: &str) -> Song {
        let mut song = Song::default();
        song.file = format!("video_{}", title);
        song.metadata.insert("title".into(), vec![title.to_string()]);
        song
    }

    #[test]
    fn test_add_to_queue() {
        let queue = QueueService::new();
        let song = create_test_song("test");
        
        let id = queue.add(song, None);
        assert_eq!(id, 1);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_add_with_position() {
        let queue = QueueService::new();
        
        queue.add(create_test_song("first"), None);
        queue.add(create_test_song("second"), None);
        queue.add(create_test_song("inserted"), Some(1));
        
        assert_eq!(queue.len(), 3);
        let song = queue.get_by_index(1).unwrap();
        assert_eq!(song.metadata.get("title").unwrap()[0], "inserted");
    }

    #[test]
    fn test_remove_from_queue() {
        let queue = QueueService::new();
        let id = queue.add(create_test_song("test"), None);
        
        assert!(queue.remove(id).is_ok());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        
        queue.clear();
        assert_eq!(queue.len(), 0);
        assert!(queue.current_index().is_none());
    }

    #[test]
    fn test_current_index() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        
        assert!(queue.current_index().is_none());
        
        queue.set_current(Some(0));
        assert_eq!(queue.current_index(), Some(0));
    }

    #[test]
    fn test_next_index() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        queue.add(create_test_song("three"), None);
        
        queue.set_current(Some(0));
        assert_eq!(queue.next_index(), Some(1));
        
        queue.set_current(Some(2));
        assert_eq!(queue.next_index(), None);
    }

    #[test]
    fn test_previous_index() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        
        queue.set_current(Some(1));
        assert_eq!(queue.previous_index(), Some(0));
        
        queue.set_current(Some(0));
        assert_eq!(queue.previous_index(), None);
    }

    #[test]
    fn test_get_by_id() {
        let queue = QueueService::new();
        let id = queue.add(create_test_song("test"), None);
        
        let song = queue.get_by_id(id).unwrap();
        assert_eq!(song.metadata.get("title").unwrap()[0], "test");
    }

    #[test]
    fn test_get_all() {
        let queue = QueueService::new();
        queue.add(create_test_song("one"), None);
        queue.add(create_test_song("two"), None);
        
        let all = queue.get_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_move_song() {
        let queue = QueueService::new();
        let id1 = queue.add(create_test_song("first"), None);
        let id2 = queue.add(create_test_song("second"), None);
        
        assert!(queue.move_song(id1, id2).is_ok());
    }

    #[test]
    fn test_is_empty() {
        let queue = QueueService::new();
        assert!(queue.is_empty());
        
        queue.add(create_test_song("test"), None);
        assert!(!queue.is_empty());
    }
}
