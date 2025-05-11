use batched_queue::{BatchedQueue, BatchedQueueTrait};
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn bench_single_producer(c: &mut Criterion) {
    let mut group = c.benchmark_group("Single Producer");

    for batch_size in [10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("push", batch_size),
            batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    let queue = BatchedQueue::<usize>::new(batch_size).unwrap();
                    let sender = queue.create_sender();

                    // Push twice the batch size of items
                    for i in 0..batch_size * 2 {
                        sender.push(black_box(i)).unwrap();
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_multiple_producers(c: &mut Criterion) {
    let mut group = c.benchmark_group("Multiple Producers");

    for thread_count in [2, 4, 8, 16].iter() {
        group.bench_with_input(
            BenchmarkId::new("push_concurrent", thread_count),
            thread_count,
            |b, &thread_count| {
                b.iter(|| {
                    let queue = Arc::new(BatchedQueue::<usize>::new(100).unwrap());
                    let mut handles = Vec::new();

                    // Spawn multiple producer threads
                    for t in 0..thread_count {
                        let q = queue.clone();
                        handles.push(thread::spawn(move || {
                            let sender = q.create_sender();

                            // Each thread pushes 1000 items
                            for i in 0..1000 {
                                sender.push(black_box(i + t * 1000)).unwrap();
                            }
                        }));
                    }

                    // Wait for all producers to finish
                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_producer_consumer(c: &mut Criterion) {
    let mut group = c.benchmark_group("Producer Consumer");

    group.bench_function("concurrent_push_next_batch", |b| {
        b.iter(|| {
            let queue = Arc::new(BatchedQueue::<usize>::new(100).unwrap());
            let q_clone = queue.clone();

            // Producer thread
            let producer = thread::spawn(move || {
                let sender = q_clone.create_sender();

                // Push 10,000 items
                for i in 0..10_000 {
                    sender.push(black_box(i)).unwrap();
                }

                // Ensure any remaining items are flushed
                sender.flush().unwrap();
            });

            // Consumer thread
            let consumer = thread::spawn(move || {
                let mut received = 0;

                // Consume all batches
                while received < 10_000 {
                    if let Ok(Some(batch)) = queue.try_next_batch() {
                        received += batch.len();
                    }

                    // Sleep a bit to allow batches to form
                    if received < 10_000 {
                        thread::sleep(Duration::from_micros(10));
                    }
                }
            });

            // Wait for both threads to finish
            producer.join().unwrap();
            consumer.join().unwrap();
        });
    });

    group.finish();
}

fn bench_backpressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("Backpressure");

    group.bench_function("bounded_queue", |b| {
        b.iter(|| {
            // Create a queue with small capacity to test backpressure
            let queue = Arc::new(BatchedQueue::<usize>::new_bounded(10, 5).unwrap());
            let q_clone = queue.clone();

            // Create a shared "done" flag to help threads coordinate shutdown
            let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let done_consumer = done.clone();

            // Slow consumer thread - start this first to ensure it's ready to consume
            let consumer = thread::spawn(move || {
                let mut received = 0;
                let target = 1_000; // Match producer's count

                // Consume items, but with artificial delay
                while received < target && !done_consumer.load(std::sync::atomic::Ordering::Relaxed)
                {
                    if let Ok(Some(batch)) = queue.try_next_batch() {
                        received += batch.len();

                        // Simulate slower processing - but not too slow
                        thread::sleep(Duration::from_micros(10));
                    } else {
                        // Small sleep when no batch is available
                        thread::sleep(Duration::from_micros(1));
                    }
                }

                // Make sure we consume any remaining batches
                while let Ok(Some(batch)) = queue.try_next_batch() {
                    received += batch.len();
                }

                received
            });

            // Give consumer thread a moment to start
            thread::sleep(Duration::from_micros(10));

            // Fast producer thread
            let producer = thread::spawn(move || {
                let sender = q_clone.create_sender();
                let mut pushed = 0;

                // Push 1,000 items with backpressure handling
                for i in 0..1_000 {
                    // Use try_push to avoid blocking, with retry logic
                    let mut retry_count = 0;
                    while retry_count < 1000 {
                        // Limit retries to prevent infinite loop
                        match sender.try_push(black_box(i)) {
                            Ok(_) => {
                                pushed += 1;
                                break;
                            }
                            Err(e) => {
                                if !matches!(e, batched_queue::BatchedQueueError::ChannelFull) {
                                    panic!("Unexpected error: {:?}", e);
                                }
                                // Small sleep before retrying - this is crucial
                                thread::sleep(Duration::from_micros(1));
                                retry_count += 1;
                            }
                        }
                    }

                    // If we hit max retries, let's move on to avoid deadlock
                    if retry_count >= 2000 {
                        println!("Warning: Max retries reached at item {}", i);
                        break;
                    }
                }

                // Ensure any remaining items are flushed
                // But with retry logic for bounded queues
                let mut retry_count = 0;
                while retry_count < 100 {
                    match sender.try_flush() {
                        Ok(_) => break,
                        Err(e) => {
                            if !matches!(e, batched_queue::BatchedQueueError::ChannelFull) {
                                panic!("Unexpected error during flush: {:?}", e);
                            }
                            thread::sleep(Duration::from_micros(10));
                            retry_count += 1;
                        }
                    }
                }

                // Signal to consumer that we're done
                done.store(true, std::sync::atomic::Ordering::Relaxed);

                pushed
            });

            // Wait for both threads to finish with timeout
            let producer_result = producer.join().unwrap();
            let consumer_result = consumer.join().unwrap();

            // Verify correct operation
            assert!(producer_result > 0, "Producer didn't push any items");
            assert!(consumer_result > 0, "Consumer didn't receive any items");
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_producer,
    bench_multiple_producers,
    bench_producer_consumer,
    bench_backpressure
);
criterion_main!(benches);
