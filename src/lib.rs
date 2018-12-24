use std::collections::{
    LinkedList,
};

use std::vec::Vec;

use ccs::{
    ThreadKey,
    Network,
};

/// Thread scheduling information.
pub struct ThreadSched {
    key: ThreadKey,
}

/// Type to identify core.
pub type CoreKey = usize;

pub trait Cpu {

    /// Initialize CPU controller structures.
    fn init() -> Self;

    /// How many cores are available in the system.
    fn core_count(&self) -> usize;

    /// Get key of core where scheduler executes.
    fn master_core(&self) -> CoreKey;

    /// Attach thread to given core.
    fn attach_thread(&mut self, core: CoreKey, thread: &ThreadSched);

    /// Set next timer to countdown the time. Given time is in Hz = 1/freq sec.
    fn set_timer(&mut self, freq: u16);

    /// Join thread and pause execution of scheduler code.
    fn join(&mut self);
}

pub struct Sched<C: Cpu> {

    freq: u16,

    threads: Vec<ThreadSched>,

    cur_thread_id: usize,

    /// CCS network controller.
    network: Network,

    cpu: Box<C>,
}

impl<C> Sched<C>
        where C: Cpu {

    pub fn new() -> Self {
        Default::default()
    }

    /// Frequency of internal timer that shows how often to change threads.
    /// Measured in Hz.
    pub fn frequency(&self) -> u16 {
        self.freq.clone()
    }

    /// Vec of threads that are sleeping and waiting to be executed.
    pub fn threads(&self) -> &Vec<ThreadSched> {
        &self.threads
    }

    /// Start scheduling tasks and switching threads.
    pub fn start(&mut self) -> ! {
        loop {
            self.switch_threads();
            self.reset_timer();
            self.join_thread();
        }
    }

    /// Switch threads command.
    fn switch_threads(&mut self) {
        let core_count = self.cpu.core_count();
        let threads_len = self.threads.len();

        // How many threads to switch.
        let switch_count = if core_count > threads_len {
            threads_len
        } else {
            core_count
        };

        let mut remain = switch_count;
        let mut cur_core = 0;
        while remain > 0 {
            // Get next thread id.
            let prev_id = self.cur_thread_id;
            let next_id = self.cur_thread_id + 1;
            let next_id = if next_id > threads_len {
                0
            } else {
                next_id
            };
            self.cur_thread_id = next_id;

            // Attach thread to core.
            self.cpu.attach_thread(cur_core, &self.threads[next_id]);

            // Update state of the threads in the network.
            self.network.active_thread(self.threads[next_id].key());
            self.network.sleep_thread(self.threads[prev_id].key());

            cur_core += 1;
            remain -= 1;
        }
    }

    /// Restart timer to count time for next switch.
    fn reset_timer(&mut self) {
        self.cpu.set_timer(self.freq);
    }

    /// Join the thread on master core and thus pause execution of scheduler
    /// code until timer fires interrupt.
    fn join_thread(&mut self) {
        self.cpu.join();
    }
}

impl<C> Default for Sched<C>
        where C: Cpu {

    fn default() -> Self {
        Sched {
            freq: 100,
            threads: Default::default(),
            cur_thread_id: Default::default(),
            active_thread_ids: Default::default(),
            network: Default::default(),
            cpu: Box::new(C::init()),
        }
    }
}

impl ThreadSched {

    pub fn new(key: ThreadKey) -> Self {
        ThreadSched {
            key,
        }
    }

    pub fn key(&self) -> &ThreadKey {
        &self.key
    }
}

#[cfg(test)]
mod tests {
}
