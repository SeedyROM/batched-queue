# `batched-queue`

A high-performance, highly-concurrent batched queue implementation for Rust.

[![GitHub](https://img.shields.io/badge/github-batched--queue-8da0cb?logo=github)](https://github.com/SeedyROM/batched-queue)
[![Crates.io](https://img.shields.io/crates/v/batched-queue.svg)](https://crates.io/crates/batched-queue)
[![Documentation](https://docs.rs/batched-queue/badge.svg)](https://docs.rs/batched-queue)
[![crates.io version](https://img.shields.io/crates/l/batched-queue.svg)](https://github.com/SeedyROM/batched-queue/blob/main/LICENSE)

## Overview

`batched-queue` provides an efficient way to collect individual items into batches for processing, which can significantly improve throughput in high-volume systems. The library offers both synchronous and asynchronous implementations, making it suitable for a wide range of applications.

## Features

- **Automatic Batching**: Collects individual items into batches of configurable size
- **Multiple Implementations**:
  - Synchronous (default) using `parking_lot` and `crossbeam-channel`
  - Asynchronous using `tokio` (via feature flag)
- **Thread-safe**: Designed for concurrent usage with multiple producers and consumers
- **Backpressure Control**: Optional bounded queue to manage memory usage
- **Flexible Retrieval**: Blocking, non-blocking, and timeout-based batch retrieval methods
- **High Performance**: Optimized for low contention and high throughput

## Installation

Add `batched-queue` to your `Cargo.toml`:

```toml
[dependencies]
batched-queue = "0.1.0"
```

### Feature Flags

- `sync` (default): Enables the synchronous implementation using `parking_lot` and `crossbeam-channel`
- `async`: Enables the asynchronous implementation using `tokio`

To use the async implementation:

```toml
[dependencies]
batched-queue = { version = "0.1.0", default-features = false, features = ["async"] }
```

## Usage Examples

### Basic Usage

```rust
use batched_queue::{BatchedQueue, BatchedQueueTrait};

fn main() {
    // Create a queue with batch size of 10
    let queue = BatchedQueue::new(10).expect("Failed to create queue");
    
    // Create a sender that can be shared across threads
    let sender = queue.create_sender();
    
    // Push items to the queue
    for i in 0..25 {
        sender.push(i).expect("Failed to push item");
    }
    
    // Flush any remaining items that haven't formed a complete batch
    sender.flush().expect("Failed to flush queue");
    
    // Process batches
    while let Ok(batch) = queue.try_next_batch() {
        println!("Processing batch of {} items", batch.len());
        for item in batch {
            println!("  Item: {}", item);
        }
    }
}
```

### Multi-threaded Usage

```rust
use batched_queue::{BatchedQueue, BatchedQueueTrait};
use std::thread;
use std::time::Duration;

fn main() {
    // Create a queue with batch size of 5
    let queue = BatchedQueue::new(5).expect("Failed to create queue");
    
    // Create a sender that can be shared across threads
    let sender = queue.create_sender();
    
    // Producer thread
    let producer = thread::spawn(move || {
        for i in 0..100 {
            sender.push(i).expect("Failed to push item");
            thread::sleep(Duration::from_millis(5));
        }
        sender.flush().expect("Failed to flush queue"); // Send any remaining items
    });
    
    // Consumer thread
    let consumer = thread::spawn(move || {
        let mut all_items = Vec::new();
        
        // Process batches as they become available
        while all_items.len() < 100 {
            if let Some(batch) = queue.next_batch_timeout(Duration::from_millis(100)) {
                println!("Received batch of {} items", batch.len());
                all_items.extend(batch);
            }
        }
        
        all_items
    });
    
    // Wait for threads to complete
    producer.join().unwrap();
    let result = consumer.join().unwrap();
    
    println!("Processed {} items in total", result.len());
}
```

### Bounded Queue with Backpressure

```rust
use batched_queue::{BatchedQueue, BatchedQueueTrait};

fn main() {
    // Create a queue with batch size 10 and at most 5 batches in the channel
    let queue = BatchedQueue::new_bounded(10, 5).expect("Failed to create bounded queue");
    
    // When the channel is full, producers will block when attempting to send a full batch
    // This provides automatic backpressure to control memory usage
    
    let sender = queue.create_sender();
    
    // ... use queue as normal
}
```

## Performance

`batched-queue` is designed for high performance in concurrent environments:

- Optimized for minimal lock contention
- Uses efficient lock-free algorithms where possible, highly concurrent otherwise
- Can achieve millions of items per second on modern hardware

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.