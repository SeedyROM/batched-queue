pub trait BatchedQueueTrait<T> {
    fn new(batch_size: usize) -> Self;
    fn push(&self, item: T);
    fn try_next_batch(&self) -> Option<Vec<T>>;
    fn next_batch(&self) -> Option<Vec<T>>;
    fn next_batch_timeout(&self, timeout: std::time::Duration) -> Option<Vec<T>>;
    fn len(&self) -> usize;
    fn capacity(&self) -> usize;
    fn flush(&self) -> bool;
    fn is_empty(&self) -> bool;
}

#[cfg(feature = "sync")]
pub mod sync {
    use super::BatchedQueueTrait;
    use crossbeam_channel as channel;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub struct BatchedQueue<T> {
        batch_size: usize,
        current_batch: Arc<Mutex<Vec<T>>>,
        batch_receiver: channel::Receiver<Vec<T>>,
        batch_sender: channel::Sender<Vec<T>>,
        item_count: Arc<AtomicUsize>,
    }

    impl<T: Send + 'static> BatchedQueue<T> {
        pub fn new(batch_size: usize) -> Self {
            let (batch_sender, batch_receiver) = channel::unbounded();
            Self {
                batch_size,
                current_batch: Arc::new(Mutex::new(Vec::with_capacity(batch_size))),
                batch_receiver,
                batch_sender,
                item_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        // Create a queue with a bounded channel for backpressure
        pub fn new_bounded(batch_size: usize, max_batches: usize) -> Self {
            let (batch_sender, batch_receiver) = channel::bounded(max_batches);
            Self {
                batch_size,
                current_batch: Arc::new(Mutex::new(Vec::with_capacity(batch_size))),
                batch_receiver,
                batch_sender,
                item_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub fn create_sender(&self) -> BatchedQueueSender<T> {
            BatchedQueueSender {
                batch_size: self.batch_size,
                current_batch: self.current_batch.clone(),
                batch_sender: self.batch_sender.clone(),
                item_count: self.item_count.clone(),
            }
        }

        // Can be called when shutting down to ensure all senders are dropped
        pub fn close_queue(&self) -> Vec<T> {
            // Take any items left in the current batch
            let mut batch = self.current_batch.lock();
            std::mem::replace(&mut *batch, Vec::with_capacity(0))
        }
    }

    impl<T: Send + 'static> BatchedQueueTrait<T> for BatchedQueue<T> {
        fn new(batch_size: usize) -> Self {
            Self::new(batch_size)
        }

        fn push(&self, item: T) {
            let mut batch = self.current_batch.lock();
            batch.push(item);

            // Update item count atomically
            let count = self.item_count.fetch_add(1, Ordering::SeqCst);

            // Check if batch is full (accounting for potential race conditions)
            if count % self.batch_size == self.batch_size - 1 {
                // Batch full, create a new one and send the full batch
                let full_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));

                // Send the full batch - will block if channel is bounded and full
                let _ = self.batch_sender.send(full_batch);
            }
        }

        fn try_next_batch(&self) -> Option<Vec<T>> {
            match self.batch_receiver.try_recv() {
                Ok(batch) => Some(batch),
                Err(_) => None,
            }
        }

        fn next_batch(&self) -> Option<Vec<T>> {
            match self.batch_receiver.recv() {
                Ok(batch) => Some(batch),
                Err(_) => None,
            }
        }

        fn next_batch_timeout(&self, timeout: std::time::Duration) -> Option<Vec<T>> {
            match self.batch_receiver.recv_timeout(timeout) {
                Ok(batch) => Some(batch),
                Err(_) => None,
            }
        }

        fn len(&self) -> usize {
            self.item_count.load(Ordering::SeqCst)
        }

        fn capacity(&self) -> usize {
            self.batch_size
        }

        fn flush(&self) -> bool {
            let mut batch = self.current_batch.lock();
            if !batch.is_empty() {
                let partial_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));
                self.batch_sender.send(partial_batch).is_ok()
            } else {
                true
            }
        }

        fn is_empty(&self) -> bool {
            self.batch_receiver.is_empty() && self.current_batch.lock().is_empty()
        }
    }

    pub struct BatchedQueueSender<T> {
        batch_size: usize,
        current_batch: Arc<Mutex<Vec<T>>>,
        batch_sender: channel::Sender<Vec<T>>,
        item_count: Arc<AtomicUsize>,
    }

    impl<T: Send + 'static> Clone for BatchedQueueSender<T> {
        fn clone(&self) -> Self {
            Self {
                batch_size: self.batch_size,
                current_batch: self.current_batch.clone(),
                batch_sender: self.batch_sender.clone(),
                item_count: self.item_count.clone(),
            }
        }
    }

    impl<T: Send + Clone + 'static> BatchedQueueSender<T> {
        pub fn push(&self, item: T) {
            let mut batch = self.current_batch.lock();
            batch.push(item);

            let count = self.item_count.fetch_add(1, Ordering::SeqCst);

            if count % self.batch_size == self.batch_size - 1 {
                let full_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));

                let _ = self.batch_sender.send(full_batch);
            }
        }

        // Non-blocking push that returns false if the channel is full
        pub fn try_push(&self, item: T) {
            let mut batch = self.current_batch.lock();
            batch.push(item);

            let count = self.item_count.fetch_add(1, Ordering::SeqCst);

            if count % self.batch_size == self.batch_size - 1 {
                let full_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));

                // Try to send the batch
                if self.batch_sender.try_send(full_batch.clone()).is_err() {
                    // If channel is full, put the batch back
                    *batch = full_batch;
                    // We didn't actually send a batch, so decrement the count
                    self.item_count.fetch_sub(1, Ordering::SeqCst);
                }
            }
        }

        pub fn flush(&self) -> bool {
            let mut batch = self.current_batch.lock();
            if !batch.is_empty() {
                let partial_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));
                self.batch_sender.send(partial_batch).is_ok()
            } else {
                true
            }
        }

        // Try to flush without blocking, returns false if channel is full
        pub fn try_flush(&self) -> bool {
            let mut batch = self.current_batch.lock();
            if !batch.is_empty() {
                let partial_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));
                self.batch_sender.try_send(partial_batch).is_ok()
            } else {
                true
            }
        }
    }

    // For testing thread-safe behavior
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::thread;
        use std::time::Duration;

        #[test]
        fn test_multithreaded() {
            let queue = BatchedQueue::<i32>::new(10);
            let sender1 = queue.create_sender();
            let sender2 = queue.create_sender();

            // Thread 1: Push numbers 0-49
            let t1 = thread::spawn(move || {
                for i in 0..50 {
                    sender1.push(i);
                    thread::sleep(Duration::from_millis(1));
                }
                sender1.flush();
            });

            // Thread 2: Push numbers 100-149
            let t2 = thread::spawn(move || {
                for i in 100..150 {
                    sender2.push(i);
                    thread::sleep(Duration::from_millis(1));
                }
                sender2.flush();
            });

            // Consumer thread: Collect all batches
            let t3 = thread::spawn(move || {
                let mut all_items = Vec::new();

                // Collect for a reasonable amount of time
                for _ in 0..15 {
                    if let Some(batch) = queue.try_next_batch() {
                        all_items.extend(batch);
                    }
                    thread::sleep(Duration::from_millis(10));
                }

                // Make sure we got all remaining batches
                while let Some(batch) = queue.try_next_batch() {
                    all_items.extend(batch);
                }

                all_items
            });

            // Wait for producer threads to finish
            t1.join().unwrap();
            t2.join().unwrap();

            // Get results from consumer
            let result = t3.join().unwrap();

            // Verify we have all 100 items
            assert_eq!(result.len(), 100);

            // Check that we have all numbers
            let mut result_sorted = result.clone();
            result_sorted.sort();

            // Verify we got all numbers 0-49 and 100-149
            for i in 0..50 {
                assert!(result_sorted.contains(&i));
                assert!(result_sorted.contains(&(i + 100)));
            }
        }

        #[test]
        fn test_timeout() {
            let queue = BatchedQueue::<i32>::new(5);
            let sender = queue.create_sender();

            // Add 3 items (not enough to trigger a batch)
            for i in 1..4 {
                sender.push(i);
            }

            // Try to get a batch with a short timeout - should time out
            let result = queue.next_batch_timeout(Duration::from_millis(10));
            assert!(result.is_none());

            // Now flush the incomplete batch
            sender.flush();

            // Should get the batch now
            let batch = queue.next_batch_timeout(Duration::from_millis(10)).unwrap();
            assert_eq!(batch, vec![1, 2, 3]);
        }

        #[test]
        fn test_bounded_channel() {
            // Create a bounded queue with batch size 5 and max 2 batches in the channel
            let queue = BatchedQueue::new_bounded(5, 2);
            let sender = queue.create_sender();

            // Producer thread
            let handle = thread::spawn(move || {
                let mut successful_pushes = 0;
                // Try to push 20 items
                for item_idx in 0..20 {
                    // Use push which will block if the channel is full
                    sender.push(item_idx);
                    successful_pushes += 1;

                    // Add a small delay
                    if item_idx % 5 == 4 {
                        // Every 5th item, wait a bit longer
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                sender.flush();
                successful_pushes
            });

            // Consumer thread - retrieve batches to prevent deadlock
            let mut received_batches = 0;
            let mut all_items = Vec::new();

            // Receive batches while the producer is running
            while received_batches < 4 {
                // Expect 4 full batches of 5 items each
                if let Some(batch) = queue.try_next_batch() {
                    received_batches += 1;
                    all_items.extend(batch);
                }
                thread::sleep(Duration::from_millis(5));
            }

            // Wait for producer to finish
            let successful_pushes = handle.join().unwrap();

            // Receive any remaining batches
            while let Some(batch) = queue.try_next_batch() {
                all_items.extend(batch);
            }

            // Should have all 20 items
            assert_eq!(all_items.len(), 20);
            assert_eq!(successful_pushes, 20);

            // Verify we have all numbers 0-19
            let mut sorted_items = all_items.clone();
            sorted_items.sort();
            for i in 0..20 {
                assert!(sorted_items.contains(&i));
            }
        }

        #[test]
        fn backpressure() {
            // Create a bounded queue with backpressure
            let queue = BatchedQueue::new_bounded(5, 1); // Only 1 batch in the channel
            let sender = queue.create_sender();

            // Fill the first batch and send it
            for i in 0..5 {
                sender.push(i);
            }
            // Now the batch is automatically sent because it's full

            // Create a partial second batch
            for i in 5..8 {
                sender.push(i);
            }

            // At this point, we have one full batch in the channel and a partial batch in current_batch

            // Get the first batch to make room in the channel
            let batch = queue.next_batch().unwrap();
            assert_eq!(batch, vec![0, 1, 2, 3, 4]);

            // Now flush the partial batch - this should succeed
            assert!(sender.try_flush());

            // And we should get the second batch
            let batch = queue.next_batch_timeout(Duration::from_millis(50)).unwrap();
            assert_eq!(batch, vec![5, 6, 7]);
        }

        #[cfg(test)]
        mod stress_tests {
            use super::*;
            use std::collections::HashSet;
            use std::sync::atomic::{AtomicUsize, Ordering};
            use std::sync::{Arc, Barrier};
            use std::thread;
            use std::time::{Duration, Instant};

            #[test]
            fn stress_test_batched_queue() {
                // Configuration parameters
                const BATCH_SIZE: usize = 100;
                const CHANNEL_CAPACITY: usize = 10;
                const PRODUCER_COUNT: usize = 64;
                const ITEMS_PER_PRODUCER: usize = 10_000;
                const CONSUMER_COUNT: usize = 4;

                // Wrap the queue in an Arc so it can be safely shared between threads
                let queue = Arc::new(BatchedQueue::new_bounded(BATCH_SIZE, CHANNEL_CAPACITY));

                // Setup synchronization primitives
                let start_barrier = Arc::new(Barrier::new(PRODUCER_COUNT + CONSUMER_COUNT + 1));
                let total_expected_items = PRODUCER_COUNT * ITEMS_PER_PRODUCER;
                let processed_items = Arc::new(AtomicUsize::new(0));

                // Tracking data structures for verification
                let all_produced_items = Arc::new(parking_lot::Mutex::new(HashSet::new()));
                let all_consumed_items = Arc::new(parking_lot::Mutex::new(HashSet::new()));

                // Track performance metrics
                let producer_times = Arc::new(parking_lot::Mutex::new(Vec::new()));
                let consumer_times = Arc::new(parking_lot::Mutex::new(Vec::new()));

                // Create and launch producer threads
                let producer_handles: Vec<_> = (0..PRODUCER_COUNT)
                    .map(|producer_id| {
                        let queue_sender = queue.create_sender();
                        let start = start_barrier.clone();
                        let produced = all_produced_items.clone();
                        let producer_timing = producer_times.clone();

                        thread::spawn(move || {
                            // Wait for all threads to be ready
                            start.wait();
                            let start_time = Instant::now();

                            // Producer offset ensures each producer generates unique values
                            let offset = producer_id * ITEMS_PER_PRODUCER;

                            // Track items we produced in this thread
                            let mut local_produced = HashSet::new();

                            for i in 0..ITEMS_PER_PRODUCER {
                                let item = offset + i;
                                queue_sender.push(item);
                                local_produced.insert(item);

                                // Occasionally sleep to create more contention patterns
                                if i % 1000 == 0 {
                                    thread::sleep(Duration::from_micros(10));
                                }
                            }

                            // Ensure final batch is sent
                            queue_sender.flush();

                            // Record items this producer generated
                            let mut global_produced = produced.lock();
                            for item in local_produced {
                                global_produced.insert(item);
                            }

                            let elapsed = start_time.elapsed();
                            producer_timing.lock().push(elapsed);

                            println!("Producer {}: Finished in {:?}", producer_id, elapsed);
                        })
                    })
                    .collect();

                // Create and launch consumer threads
                let consumer_handles: Vec<_> = (0..CONSUMER_COUNT)
                    .map(|consumer_id| {
                        let queue = queue.clone(); // Clone the Arc, not the queue itself
                        let start = start_barrier.clone();
                        let processed = processed_items.clone();
                        let consumed = all_consumed_items.clone();
                        let consumer_timing = consumer_times.clone();

                        thread::spawn(move || {
                            // Wait for all threads to be ready
                            start.wait();
                            let start_time = Instant::now();

                            // Track items consumed by this thread
                            let mut local_consumed = HashSet::new();
                            let mut batches_processed = 0;

                            loop {
                                // Try to get a batch with timeout
                                if let Some(batch) =
                                    queue.next_batch_timeout(Duration::from_millis(100))
                                {
                                    batches_processed += 1;
                                    let batch_size = batch.len();

                                    // Process each item in the batch
                                    for item in batch {
                                        local_consumed.insert(item);
                                    }

                                    // Update total processed count
                                    let current = processed.fetch_add(batch_size, Ordering::SeqCst);

                                    // Check if we've processed all expected items
                                    if current + batch_size >= total_expected_items {
                                        break;
                                    }
                                } else if processed.load(Ordering::SeqCst) >= total_expected_items {
                                    // No more batches and we've processed all expected items
                                    break;
                                }

                                // Occasionally check if we're done to avoid waiting for full timeout
                                if processed.load(Ordering::SeqCst) >= total_expected_items {
                                    break;
                                }
                            }

                            // Record the items this consumer processed
                            let mut global_consumed = consumed.lock();
                            for item in local_consumed {
                                global_consumed.insert(item);
                            }

                            let elapsed = start_time.elapsed();
                            consumer_timing.lock().push(elapsed);

                            println!(
                                "Consumer {}: Processed {} batches in {:?}",
                                consumer_id, batches_processed, elapsed
                            );
                        })
                    })
                    .collect();

                // Start the test
                println!(
                    "Starting stress test with {} producers and {} consumers",
                    PRODUCER_COUNT, CONSUMER_COUNT
                );
                println!(
                    "Each producer will generate {} items with batch size {}",
                    ITEMS_PER_PRODUCER, BATCH_SIZE
                );

                let overall_start = Instant::now();
                start_barrier.wait();

                // Wait for all producers to finish
                for handle in producer_handles {
                    handle.join().unwrap();
                }

                println!("All producers finished");

                // Wait for all consumers to finish
                for handle in consumer_handles {
                    handle.join().unwrap();
                }

                let overall_elapsed = overall_start.elapsed();
                println!("All consumers finished");
                println!("Overall test time: {:?}", overall_elapsed);

                // Verify results
                let produced = all_produced_items.lock();
                let consumed = all_consumed_items.lock();

                println!("Items produced: {}", produced.len());
                println!("Items consumed: {}", consumed.len());

                // Verify all produced items were consumed
                assert_eq!(
                    produced.len(),
                    total_expected_items,
                    "Number of produced items doesn't match expected"
                );
                assert_eq!(
                    consumed.len(),
                    total_expected_items,
                    "Number of consumed items doesn't match expected"
                );

                for item in produced.iter() {
                    assert!(
                        consumed.contains(item),
                        "Item {} was produced but not consumed",
                        item
                    );
                }

                // Calculate performance metrics
                let producer_times = producer_times.lock();
                let consumer_times = consumer_times.lock();

                let avg_producer_time = producer_times.iter().map(|d| d.as_millis()).sum::<u128>()
                    / producer_times.len() as u128;

                let avg_consumer_time = consumer_times.iter().map(|d| d.as_millis()).sum::<u128>()
                    / consumer_times.len() as u128;

                let throughput =
                    total_expected_items as f64 / (overall_elapsed.as_millis() as f64 / 1000.0);

                println!("Average producer time: {}ms", avg_producer_time);
                println!("Average consumer time: {}ms", avg_consumer_time);
                println!("Throughput: {:.2} items/second", throughput);
            }
        }
    }
}

#[cfg(feature = "sync")]
pub use sync::BatchedQueue;
