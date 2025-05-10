//! # `batched-queue`
//!
//! A high-performance, highly-concurrent batched queue implementation for Rust.
//!
//! `batched-queue` provides an efficient way to collect individual items into batches
//! for processing, which can significantly improve throughput in high-volume systems.
//!
//! ## Features
//!
//! - **Batching**: Automatically collects items into batches of a configurable size
//! - **Thread-safe**: Designed for concurrent usage with multiple producers and consumers
//! - **Backpressure**: Optional bounded queue to control memory usage
//! - **Flexible retrieval**: Blocking, non-blocking, and timeout-based batch retrieval
//! - **Multiple implementations**: Synchronous (default) and asynchronous modes via feature flags
//!
//! ## Usage
//!
//! By default, the crate provides a synchronous implementation:
//!
//! ```rust
//! use batched_queue::{BatchedQueue, BatchedQueueTrait};
//!
//! // Create a queue with batch size of 10
//! let queue = BatchedQueue::new(10).expect("Failed to create queue");
//!
//! // Create a sender that can be shared across threads
//! let sender = queue.create_sender();
//!
//! // Push items to the queue (in real usage, this would be in another thread)
//! for i in 0..25 {
//!     sender.push(i).expect("Failed to push item");
//! }
//!
//! // Flush any remaining items that haven't formed a complete batch
//! sender.flush().expect("Failed to flush");
//!
//! // Process batches
//! while let Some(batch) = queue.try_next_batch().expect("Failed to get batch") {
//!     println!("Processing batch of {} items", batch.len());
//!     for item in batch {
//!         // Process each item
//!         println!("  Item: {}", item);
//!     }
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - [`sync`] (default): Enables the synchronous implementation using [`parking_lot`] and [`crossbeam_channel`]
//! - `async`: Enables the asynchronous implementation using tokio

use std::time::Duration;
use thiserror::Error;

// Export the sync BatchedQueue by default
#[cfg(feature = "sync")]
pub use sync::BatchedQueue;

/// Error type for a batched queue.
#[derive(Error, Debug, Clone)]
pub enum BatchedQueueError {
    #[error("Channel is full (backpressure limit reached)")]
    ChannelFull,

    #[error("Channel is disconnected (all receivers dropped)")]
    Disconnected,

    #[error("Operation timed out after {0:?}")]
    Timeout(Duration),

    #[error("Queue capacity exceeded: tried to add more than {max_capacity} items")]
    CapacityExceeded { max_capacity: usize },

    #[error("Invalid batch size: {0}")]
    InvalidBatchSize(usize),

    #[error("Failed to send batch: {0}")]
    SendError(String),

    #[error("Failed to receive batch: {0}")]
    ReceiveError(String),
}

/// Contextual information about [`BatchedQueueError`].
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub operation: String,
    pub queue_info: String,
}

impl BatchedQueueError {
    pub fn timeout(duration: Duration) -> Self {
        BatchedQueueError::Timeout(duration)
    }

    pub fn capacity_exceeded(max_capacity: usize) -> Self {
        BatchedQueueError::CapacityExceeded { max_capacity }
    }
}

/// Defines the common interface for batched queue implementations.
///
/// This trait provides methods for adding items to a queue, retrieving
/// batches of items, and checking queue status. All implementations must
/// handle the buffering of items until they form complete batches, and
/// provide mechanisms for flushing partial batches when needed.
///
/// # Examples
///
/// ```
/// use batched_queue::{BatchedQueue, BatchedQueueTrait};
///
/// // Create a queue with batch size of 10
/// let queue = BatchedQueue::new(10).expect("Failed to create queue");
///
/// // Create a sender that can be shared across threads
/// let sender = queue.create_sender();
///
/// // Push items to the queue (in real usage, this would be in another thread)
/// for i in 0..25 {
///     sender.push(i).expect("Failed to push item");
/// }
///
/// // Flush any remaining items that haven't formed a complete batch
/// sender.flush().expect("Failed to flush");
///
/// // Process batches
/// while let Some(batch) = queue.try_next_batch().expect("Failed to get batch") {
///     println!("Processing batch of {} items", batch.len());
///     for item in batch {
///         // Process each item
///         println!("  Item: {}", item);
///     }
/// }
/// ```
pub trait BatchedQueueTrait<T> {
    /// Returns the current number of items in the queue.
    fn len(&self) -> usize;

    /// Returns the maximum number of items a batch can hold.
    fn capacity(&self) -> usize;

    /// Returns `true` if the queue has no items waiting to be processed.
    fn is_empty(&self) -> bool;

    /// Adds an item to the queue.
    ///
    /// If adding this item causes the current batch to reach the configured
    /// batch size, the batch will be automatically sent for processing.
    ///
    /// # Arguments
    ///
    /// * `item` - The item to add to the queue
    ///
    /// # Errors
    ///
    /// Returns `BatchedQueueError::Disconnected` if the receiving end has been dropped,
    /// or other implementation-specific errors.
    fn push(&self, item: T) -> Result<(), BatchedQueueError>;

    /// Attempts to retrieve the next batch of items without blocking.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(batch))` - A batch of items is available
    /// * `Ok(None)` - No batch is currently available
    ///
    /// # Errors
    ///
    /// Returns `BatchedQueueError::Disconnected` if the sending end has been dropped,
    /// or other implementation-specific errors.
    fn try_next_batch(&self) -> Result<Option<Vec<T>>, BatchedQueueError>;

    /// Retrieves the next batch of items, blocking until one is available.
    ///
    /// # Errors
    ///
    /// Returns `BatchedQueueError::Disconnected` if the sending end has been dropped,
    /// or other implementation-specific errors.
    fn next_batch(&self) -> Result<Vec<T>, BatchedQueueError>;

    /// Retrieves the next batch of items, blocking until one is available or until the timeout expires.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for a batch to become available
    ///
    /// # Errors
    ///
    /// Returns:
    /// * `BatchedQueueError::Timeout` if no batch becomes available within the timeout period
    /// * `BatchedQueueError::Disconnected` if the sending end has been dropped
    /// * Other implementation-specific errors
    fn next_batch_timeout(&self, timeout: std::time::Duration)
    -> Result<Vec<T>, BatchedQueueError>;

    /// Flushes any pending items into a batch, even if the batch is not full.
    ///
    /// This is useful for ensuring that all items are processed, especially
    /// during shutdown or when batches need to be processed on demand.
    ///
    /// # Errors
    ///
    /// Returns `BatchedQueueError::Disconnected` if the receiving end has been dropped,
    /// or other implementation-specific errors.
    fn flush(&self) -> Result<(), BatchedQueueError>;
}

#[cfg(feature = "sync")]
pub mod sync {
    //! Synchronous implementation of the batched queue.
    //!
    //! This module provides a thread-safe implementation using [`crossbeam_channel`]
    //! for communication and [`parking_lot::Mutex`] for low-contention locking.
    //! It is designed for high-throughput scenarios where items need to be
    //! processed in batches.

    use super::*;
    use crossbeam_channel as channel;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A thread-safe, high-performance queue that automatically batches items.
    ///
    /// [`BatchedQueue`] collects individual items until reaching the configured batch size,
    /// then automatically makes the batch available for processing. This batching approach
    /// can significantly improve throughput in high-volume systems by reducing overhead.
    ///
    /// # Examples
    ///
    /// ```
    /// use batched_queue::{BatchedQueue, BatchedQueueTrait};
    ///
    /// use std::thread;
    /// use std::time::Duration;
    ///
    /// // Create a queue with batch size of 5
    /// let queue = BatchedQueue::new(5).expect("Failed to create queue");
    ///
    /// // Create a sender that can be shared across threads
    /// let sender = queue.create_sender();
    ///
    /// // Producer thread
    /// let producer = thread::spawn(move || {
    ///     for i in 0..20 {
    ///         sender.push(i).expect("Failed to push item");
    ///         thread::sleep(Duration::from_millis(10));
    ///     }
    ///     sender.flush().expect("Failed to flush"); // Send any remaining items
    /// });
    ///
    /// // Consumer thread
    /// let consumer = thread::spawn(move || {
    ///     let mut all_items = Vec::new();
    ///     
    ///     // Process batches as they become available
    ///     while all_items.len() < 20 {
    ///         if let Ok(batch) = queue.next_batch_timeout(Duration::from_millis(100)) {
    ///             all_items.extend(batch);
    ///         }
    ///     }
    ///     
    ///     all_items
    /// });
    ///
    /// // Wait for threads to complete
    /// producer.join().unwrap();
    /// let result = consumer.join().unwrap();
    ///
    /// assert_eq!(result.len(), 20);
    /// ```
    pub struct BatchedQueue<T> {
        batch_size: usize,
        current_batch: Arc<Mutex<Vec<T>>>,
        batch_receiver: channel::Receiver<Vec<T>>,
        batch_sender: channel::Sender<Vec<T>>,
        item_count: Arc<AtomicUsize>,
    }

    impl<T: Send + 'static> BatchedQueue<T> {
        /// Creates a new batched queue with the specified batch size and an unbounded channel.
        ///
        /// # Arguments
        ///
        /// * `batch_size` - The number of items to collect before forming a batch
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// let queue = BatchedQueue::<String>::new(10).expect("Failed to create queue");
        /// ```
        pub fn new(batch_size: usize) -> Result<Self, BatchedQueueError> {
            if batch_size == 0 {
                return Err(BatchedQueueError::InvalidBatchSize(batch_size));
            }

            let (batch_sender, batch_receiver) = channel::unbounded();
            Ok(Self {
                batch_size,
                current_batch: Arc::new(Mutex::new(Vec::with_capacity(batch_size))),
                batch_receiver,
                batch_sender,
                item_count: Arc::new(AtomicUsize::new(0)),
            })
        }

        /// Creates a new batched queue with a bounded channel for backpressure.
        ///
        /// Using a bounded channel helps control memory usage by limiting the number
        /// of batches that can be queued at once. When the channel is full, producers
        /// will block when attempting to send a full batch.
        ///
        /// # Arguments
        ///
        /// * `batch_size` - The number of items to collect before forming a batch
        /// * `max_batches` - The maximum number of batches that can be queued
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// // Create a queue with batch size 10 and at most 5 batches in the channel
        /// let queue = BatchedQueue::<i32>::new_bounded(10, 5).expect("Failed to create queue");
        /// ```
        pub fn new_bounded(
            batch_size: usize,
            max_batches: usize,
        ) -> Result<Self, BatchedQueueError> {
            if batch_size == 0 {
                return Err(BatchedQueueError::InvalidBatchSize(batch_size));
            }
            if max_batches == 0 {
                return Err(BatchedQueueError::InvalidBatchSize(max_batches));
            }

            let (batch_sender, batch_receiver) = channel::bounded(max_batches);
            Ok(Self {
                batch_size,
                current_batch: Arc::new(Mutex::new(Vec::with_capacity(batch_size))),
                batch_receiver,
                batch_sender,
                item_count: Arc::new(AtomicUsize::new(0)),
            })
        }

        /// Creates a sender for this queue that can be shared across threads.
        ///
        /// Multiple senders can be created from a single queue, allowing
        /// for concurrent producers.
        ///
        /// # Returns
        ///
        /// A new [`BatchedQueueSender`] linked to this queue
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        /// use std::thread;
        ///
        /// let queue = BatchedQueue::<i32>::new(10).expect("Failed to create queue");
        ///
        /// // Create multiple senders for different threads
        /// let sender1 = queue.create_sender();
        /// let sender2 = queue.create_sender();
        ///
        /// // Use senders in different threads
        /// let t1 = thread::spawn(move || {
        ///     for i in 0..10 {
        ///         sender1.push(i).expect("Failed to push item");
        ///     }
        /// });
        ///
        /// let t2 = thread::spawn(move || {
        ///     for i in 10..20 {
        ///         sender2.push(i).expect("Failed to push item");
        ///     }
        /// });
        ///
        /// // Wait for producers to finish
        /// t1.join().unwrap();
        /// t2.join().unwrap();
        /// ```
        pub fn create_sender(&self) -> BatchedQueueSender<T> {
            BatchedQueueSender {
                batch_size: self.batch_size,
                current_batch: self.current_batch.clone(),
                batch_sender: self.batch_sender.clone(),
                item_count: self.item_count.clone(),
            }
        }

        /// Takes any items left in the current batch and returns them when shutting down.
        ///
        /// This method is useful during controlled shutdown to collect any remaining items
        /// that haven't formed a complete batch.
        ///
        /// # Returns
        ///
        /// A vector containing any items that were in the current batch
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// let queue = BatchedQueue::<i32>::new(10).expect("Failed to create queue");
        /// let sender = queue.create_sender();
        ///
        /// // Add some items, but not enough to form a complete batch
        /// for i in 0..3 {
        ///     sender.push(i).expect("Failed to push item");
        /// }
        ///
        /// // Close the queue and get remaining items
        /// let remaining = queue.close_queue();
        /// assert_eq!(remaining.len(), 3);
        /// ```
        pub fn close_queue(&self) -> Vec<T> {
            // Take any items left in the current batch
            let mut batch = self.current_batch.lock();
            std::mem::take(&mut *batch)
        }
    }

    impl<T: Send + 'static> BatchedQueueTrait<T> for BatchedQueue<T> {
        fn push(&self, item: T) -> Result<(), BatchedQueueError> {
            let mut batch = self.current_batch.lock();
            batch.push(item);

            let count = self.item_count.fetch_add(1, Ordering::SeqCst);

            if count % self.batch_size == self.batch_size - 1 {
                let full_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));

                self.batch_sender
                    .send(full_batch)
                    .map_err(|_| BatchedQueueError::Disconnected)?;
            }

            Ok(())
        }

        fn try_next_batch(&self) -> Result<Option<Vec<T>>, BatchedQueueError> {
            match self.batch_receiver.try_recv() {
                Ok(batch) => Ok(Some(batch)),
                Err(channel::TryRecvError::Empty) => Ok(None),
                Err(channel::TryRecvError::Disconnected) => Err(BatchedQueueError::Disconnected),
            }
        }

        fn next_batch(&self) -> Result<Vec<T>, BatchedQueueError> {
            self.batch_receiver
                .recv()
                .map_err(|_| BatchedQueueError::Disconnected)
        }

        fn next_batch_timeout(
            &self,
            timeout: std::time::Duration,
        ) -> Result<Vec<T>, BatchedQueueError> {
            match self.batch_receiver.recv_timeout(timeout) {
                Ok(batch) => Ok(batch),
                Err(channel::RecvTimeoutError::Timeout) => Err(BatchedQueueError::Timeout(timeout)),
                Err(channel::RecvTimeoutError::Disconnected) => {
                    Err(BatchedQueueError::Disconnected)
                }
            }
        }

        fn len(&self) -> usize {
            self.item_count.load(Ordering::SeqCst)
        }

        fn capacity(&self) -> usize {
            self.batch_size
        }

        fn flush(&self) -> Result<(), BatchedQueueError> {
            let mut batch = self.current_batch.lock();
            if !batch.is_empty() {
                let partial_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));
                self.batch_sender
                    .send(partial_batch)
                    .map_err(|_| BatchedQueueError::Disconnected)?;
            }
            Ok(())
        }

        fn is_empty(&self) -> bool {
            self.batch_receiver.is_empty() && self.current_batch.lock().is_empty()
        }
    }

    /// A sender handle for adding items to a batched queue.
    ///
    /// `BatchedQueueSender` provides methods to add items to a batched queue from multiple
    /// threads. It handles the details of batch management and automatic flushing of batches
    /// when they reach the configured size.
    ///
    /// # Examples
    ///
    /// ```
    /// use batched_queue::BatchedQueue;
    /// use std::thread;
    ///
    /// let queue = BatchedQueue::<String>::new(5).expect("Failed to create queue");
    /// let sender = queue.create_sender();
    ///
    /// // Share the sender with another thread
    /// thread::spawn(move || {
    ///     for i in 0..10 {
    ///         sender.push(format!("Item {}", i)).expect("Failed to push item");
    ///     }
    ///     
    ///     // Ensure any remaining items are sent
    ///     sender.flush().expect("Failed to flush");
    /// });
    /// ```
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
        /// Adds an item to the queue.
        ///
        /// If adding this item causes the current batch to reach the configured
        /// batch size, the batch will be automatically sent for processing.
        /// This method will block if the channel is bounded and full.
        ///
        /// # Arguments
        ///
        /// * `item` - The item to add to the queue
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// let queue = BatchedQueue::<i32>::new(5).expect("Failed to create queue");
        /// let sender = queue.create_sender();
        ///
        /// for i in 0..10 {
        ///     sender.push(i).expect("Failed to push item");
        /// }
        /// ```
        pub fn push(&self, item: T) -> Result<(), BatchedQueueError> {
            let mut batch = self.current_batch.lock();
            batch.push(item);

            let count = self.item_count.fetch_add(1, Ordering::SeqCst);

            if count % self.batch_size == self.batch_size - 1 {
                let full_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));

                self.batch_sender
                    .send(full_batch)
                    .map_err(|_| BatchedQueueError::Disconnected)?;
            }

            Ok(())
        }

        /// Attempts to add an item to the queue without blocking.
        ///
        /// This method is similar to `push`, but it will not block if the channel
        /// is bounded and full. Instead, if a full batch cannot be sent because
        /// the channel is full, the batch is kept in the current batch and will
        /// be sent on a future push or flush operation.
        ///
        /// # Arguments
        ///
        /// * `item` - The item to add to the queue
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// // Create a queue with limited capacity
        /// let queue = BatchedQueue::<i32>::new_bounded(5, 1).expect("Failed to create queue");
        /// let sender = queue.create_sender();
        ///
        /// for i in 0..20 {
        ///     sender.try_push(i);
        /// }
        /// ```
        pub fn try_push(&self, item: T) -> Result<(), BatchedQueueError> {
            let mut batch = self.current_batch.lock();
            batch.push(item);

            let count = self.item_count.fetch_add(1, Ordering::SeqCst);

            if count % self.batch_size == self.batch_size - 1 {
                let full_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));

                // Try to send the batch
                match self.batch_sender.try_send(full_batch.clone()) {
                    Ok(_) => {}
                    Err(channel::TrySendError::Full(_)) => {
                        // If channel is full, put the batch back
                        *batch = full_batch;
                        // We didn't actually send a batch, so decrement the count
                        self.item_count.fetch_sub(1, Ordering::SeqCst);
                        return Err(BatchedQueueError::ChannelFull);
                    }
                    Err(channel::TrySendError::Disconnected(_)) => {
                        return Err(BatchedQueueError::Disconnected);
                    }
                }
            }

            Ok(())
        }

        /// Flushes any pending items into a batch, even if the batch is not full.
        ///
        /// This method will block if the channel is bounded and full.
        ///
        /// # Error
        ///
        /// Returns `BatchedQueueError::Disconnected` if the receiving end has been dropped,
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// let queue = BatchedQueue::<i32>::new(10).expect("Failed to create queue");
        /// let sender = queue.create_sender();
        ///
        /// // Add some items, but not enough to form a complete batch
        /// for i in 0..3 {
        ///     sender.push(i).expect("Failed to push item");
        /// }
        ///
        /// // Flush to ensure items are sent for processing
        /// sender.flush().expect("Failed to flush");
        /// ```
        pub fn flush(&self) -> Result<(), BatchedQueueError> {
            let mut batch = self.current_batch.lock();
            if !batch.is_empty() {
                let partial_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));
                self.batch_sender
                    .send(partial_batch)
                    .map_err(|_| BatchedQueueError::Disconnected)?;
            }
            Ok(())
        }

        /// Attempts to flush any pending items without blocking.
        ///
        /// # Errors
        ///
        /// Returns `BatchedQueueError::Disconnected` if the receiving end has been dropped,
        /// or `BatchedQueueError::ChannelFull` if the channel is full.
        ///
        /// # Examples
        ///
        /// ```
        /// use batched_queue::BatchedQueue;
        ///
        /// // Create a queue with limited capacity
        /// let queue = BatchedQueue::<i32>::new_bounded(5, 1).expect("Failed to create queue");
        /// let sender = queue.create_sender();
        ///
        /// for i in 0..3 {
        ///     sender.push(i).expect("Failed to push item");
        /// }
        ///
        /// // Try to flush without blocking
        /// if !sender.try_flush().is_ok() {
        ///     println!("Channel is full, will try again later");
        /// }
        /// ```
        pub fn try_flush(&self) -> Result<(), BatchedQueueError> {
            let mut batch = self.current_batch.lock();
            if !batch.is_empty() {
                let partial_batch =
                    std::mem::replace(&mut *batch, Vec::with_capacity(self.batch_size));
                match self.batch_sender.try_send(partial_batch) {
                    Ok(_) => Ok(()),
                    Err(channel::TrySendError::Full(_)) => Err(BatchedQueueError::ChannelFull),
                    Err(channel::TrySendError::Disconnected(_)) => {
                        Err(BatchedQueueError::Disconnected)
                    }
                }
            } else {
                Ok(())
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
        fn multithreaded() {
            let queue = BatchedQueue::<i32>::new(10).expect("Failed to create queue");
            let sender1 = queue.create_sender();
            let sender2 = queue.create_sender();

            // Thread 1: Push numbers 0-49
            let t1 = thread::spawn(move || {
                for i in 0..50 {
                    sender1.push(i).expect("Failed to push item");
                    thread::sleep(Duration::from_millis(1));
                }
                sender1.flush().expect("Failed to flush");
            });

            // Thread 2: Push numbers 100-149
            let t2 = thread::spawn(move || {
                for i in 100..150 {
                    sender2.push(i).expect("Failed to push item");
                    thread::sleep(Duration::from_millis(1));
                }
                sender2.flush().expect("Failed to flush");
            });

            // Consumer thread: Collect all batches
            let t3 = thread::spawn(move || {
                let mut all_items = Vec::new();

                // Collect for a reasonable amount of time
                for _ in 0..15 {
                    if let Some(batch) = queue.try_next_batch().expect("Failed to get batch") {
                        all_items.extend(batch);
                    }
                    thread::sleep(Duration::from_millis(10));
                }

                // Make sure we got all remaining batches
                while let Some(batch) = queue.try_next_batch().expect("Failed to get batch") {
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
        fn timeout() {
            let queue = BatchedQueue::<i32>::new(5).expect("Failed to create queue");
            let sender = queue.create_sender();

            // Add 3 items (not enough to trigger a batch)
            for i in 1..4 {
                sender.push(i).unwrap();
            }

            // Try to get a batch with a short timeout - should time out
            let result = queue.next_batch_timeout(Duration::from_millis(10));
            assert!(result.is_err());

            // Now flush the incomplete batch
            sender.flush().unwrap();

            // Should get the batch now
            let batch = queue.next_batch_timeout(Duration::from_millis(10)).unwrap();
            assert_eq!(batch, vec![1, 2, 3]);
        }

        #[test]
        fn bounded_channel() {
            // Create a bounded queue with batch size 5 and max 2 batches in the channel
            let queue = BatchedQueue::new_bounded(5, 2).expect("Failed to create queue");
            let sender = queue.create_sender();

            // Producer thread
            let handle = thread::spawn(move || {
                let mut successful_pushes = 0;
                // Try to push 20 items
                for item_idx in 0..20 {
                    // Use push which will block if the channel is full
                    sender.push(item_idx).expect("Failed to push item");
                    successful_pushes += 1;

                    // Add a small delay
                    if item_idx % 5 == 4 {
                        // Every 5th item, wait a bit longer
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                sender.flush().expect("Failed to flush");
                successful_pushes
            });

            // Consumer thread - retrieve batches to prevent deadlock
            let mut received_batches = 0;
            let mut all_items = Vec::new();

            // Receive batches while the producer is running
            while received_batches < 4 {
                // Expect 4 full batches of 5 items each
                if let Some(batch) = queue.try_next_batch().expect("Failed to get batch") {
                    received_batches += 1;
                    all_items.extend(batch);
                }
                thread::sleep(Duration::from_millis(5));
            }

            // Wait for producer to finish
            let successful_pushes = handle.join().unwrap();

            // Receive any remaining batches
            while let Some(batch) = queue.try_next_batch().expect("Failed to get batch") {
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
            let queue = BatchedQueue::new_bounded(5, 1).expect("Failed to create queue"); // Only 1 batch in the channel
            let sender = queue.create_sender();

            // Fill the first batch and send it
            for i in 0..5 {
                sender.push(i).expect("Failed to push item");
            }
            // Now the batch is automatically sent because it's full

            // Create a partial second batch
            for i in 5..8 {
                sender.push(i).expect("Failed to push item");
            }

            // At this point, we have one full batch in the channel and a partial batch in current_batch

            // Get the first batch to make room in the channel
            let batch = queue.next_batch().expect("Failed to get batch");
            assert_eq!(batch, vec![0, 1, 2, 3, 4]);

            // Now flush the partial batch - this should succeed
            assert!(sender.try_flush().is_ok());

            // And we should get the second batch
            let batch = queue
                .next_batch_timeout(Duration::from_millis(50))
                .expect("Failed to get batch");
            assert_eq!(batch, vec![5, 6, 7]);
        }

        #[test]
        fn error_handling() {
            // Test invalid batch size
            let invalid_queue = BatchedQueue::<i32>::new(0);
            assert!(matches!(
                invalid_queue,
                Err(BatchedQueueError::InvalidBatchSize(0))
            ));

            // Create a queue with very limited capacity - only 1 batch in the channel
            let limited_queue = BatchedQueue::new_bounded(5, 1).expect("Failed to create queue");
            let limited_sender = limited_queue.create_sender();

            // Fill the channel with one complete batch
            for i in 0..5 {
                limited_sender
                    .push(i)
                    .expect("Should succeed for first batch");
            }

            // At this point, we have one full batch in the channel
            // Now, add items to start building a second batch
            for i in 5..9 {
                limited_sender
                    .push(i)
                    .expect("Should succeed as we're building a partial batch");
            }

            // Try to complete the second batch, which should fail with ChannelFull
            // because when it completes, it immediately tries to send it
            let result = limited_sender.try_push(9);
            assert!(matches!(result, Err(BatchedQueueError::ChannelFull)));

            // Now let's test the timeout
            // First ensure there's nothing ready in the queue by consuming the batch
            limited_queue
                .next_batch()
                .expect("Should get the first batch");

            // Now we should have nothing in the channel and only a partial batch
            // Try to get a batch with a very short timeout
            let result = limited_queue.next_batch_timeout(Duration::from_millis(1));
            assert!(matches!(result, Err(BatchedQueueError::Timeout(_))));
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
            fn batched_queue() {
                // Configuration parameters
                const BATCH_SIZE: usize = 100;
                const CHANNEL_CAPACITY: usize = 10;
                const PRODUCER_COUNT: usize = 64;
                const ITEMS_PER_PRODUCER: usize = 10_000;
                const CONSUMER_COUNT: usize = 4;

                // Wrap the queue in an Arc so it can be safely shared between threads
                let queue = Arc::new(
                    BatchedQueue::new_bounded(BATCH_SIZE, CHANNEL_CAPACITY)
                        .expect("Failed to create queue"),
                );

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
                                queue_sender.push(item).expect("Failed to push item");
                                local_produced.insert(item);

                                // Occasionally sleep to create more contention patterns
                                if i % 1000 == 0 {
                                    thread::sleep(Duration::from_micros(10));
                                }
                            }

                            // Ensure final batch is sent
                            queue_sender.flush().expect("Failed to flush");

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
                                if let Ok(batch) =
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
