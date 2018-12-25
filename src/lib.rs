use std::sync::{
    RwLock,
    RwLockReadGuard,
    RwLockWriteGuard,
};

use std::collections::{
    LinkedList,
    BTreeMap,
};

use std::vec::Vec;

use ccs::{
    ThreadKey,
    ChannelKey,
    Network,
};

/// Information from scheduler to CPU controller of how to execute join
/// operation.
pub enum JoinMode {

    /// New thread was assigned to core.
    ContextChanged,

    /// Thread runs for the first time.
    NewContext,
}

/// Depending on timer type different interrupts will raise at alarm.
pub enum TimerType {

    /// Execution timeout when threads processor time elapses.
    Execution,

    /// Wait timeout when channel signal hadn't arrived before timer alarm.
    /// Passed value indicates thread that waits.
    ChannelWait(ThreadKey),
}

/// Thread scheduling information.
pub struct ThreadSched {

    /// Thread key to identify it in the Network.
    key: ThreadKey,

    /// Additional time that was given to this thread in microseconds.
    add_time: u32,
}

/// Scheduler information per core. Each core can run it's own scheduler code
/// and thus they have their own data to refer to main scheduler structure and
/// to hold their own information related to thread executed on this core.
pub struct Core<C: Cpu> {
    sched: RwLock<Sched<C>>,

    thread: ThreadKey,
    pos: usize,

    key: CoreKey,
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

    /// Add new timer to countdown the time. Given time is in microseconds.
    fn add_timer(&mut self, time: usize, timer_type: TimerType);

    /// Join thread and exit execution of scheduler code.
    fn join(&mut self, mode: JoinMode) -> !;
}

/// Contains the queue of threads and a tree that allows to locate
/// the thread in the queue by its key.
#[derive(Default)]
struct ThreadSet {
    queue: Vec<ThreadSched>,
    map: BTreeMap<ThreadKey, usize>,
}

pub struct Sched<C: Cpu> {

    /// The time of how long a thread normally should execute before switching
    /// to other.
    time: u32,

    /// Maximal additional time that is allowed to be added for a thread
    /// in a one execution cycle. In microseconds.
    max_add_time: u32,

    /// Active threads that are executed or sleeping threads that wait for
    /// processor time.
    threads: Vec<ThreadSched>,

    /// Threads that were removed from active thread list due to they wait
    /// for external signal.
    waiting_threads: ThreadSet,

    next_thread_id: usize,

    /// CCS network controller.
    network: Network,

    cpu: Box<C>,
}

impl<C> Sched<C>
        where C: Cpu {

    pub fn new() -> Self {
        Default::default()
    }

    /// Internal timer value that shows how often to change threads.
    /// Mesured in microseconds.
    pub fn time(&self) -> u32 {
        self.time.clone()
    }

    /// Vec of threads that are sleeping and waiting to be executed.
    pub fn threads(&self) -> &Vec<ThreadSched> {
        &self.threads
    }
}

impl<C> Default for Sched<C>
        where C: Cpu {

    fn default() -> Self {
        Sched {
            time            : 5_000, // Same as 200 Hz.
            max_add_time    : 2_500,
            waiting_threads : Default::default(),
            threads         : Default::default(),
            next_thread_id  : Default::default(),
            network         : Default::default(),
            cpu             : Box::new(C::init()),
        }
    }
}

impl<C> Core<C> where C: Cpu {

    /// Interrupt of thread execution by timeout signal.
    pub fn timeout_interrupt(&mut self) -> ! {
        // Get next thread to execute.
        let (prev_pos, next_pos) = {
            let mut schedw = self.schedw();
            let next_pos = schedw.next_thread_id.clone();

            // Update next thread counter.
            let prev_pos = schedw.next_thread_id;
            schedw.next_thread_id += 1;
            if schedw.next_thread_id >= schedw.threads.len() {
                schedw.next_thread_id = 0;
            }

            (prev_pos, next_pos)
        };
        let schedr = self.schedr();
        let next = &schedr.threads[next_pos].key.clone();
        let prev = &schedr.threads[prev_pos].key.clone();
        drop(schedr);

        self.thread = next.clone();
        self.pos = next_pos;

        // Update thread state in the network.
        self.schedw().network.active_thread(&next);
        self.schedw().network.sleep_thread(&prev);

        // Update timer.
        let time = {
            let schedr = self.schedr();
            let add_time = schedr.threads[next_pos].add_time;

            let add_time = if add_time > schedr.max_add_time {
                schedr.max_add_time.clone()
            } else {
                add_time
            };

            (schedr.time + add_time) as _
        };
        self.schedw().cpu.add_timer(time, TimerType::Execution);

        self.join(JoinMode::ContextChanged);
    }

    /// Thread now waits for channel signal with timer. Time accepted in
    /// microseconds.
    pub fn channel_wait_with_timer_interrupt(&mut self, time: usize,
        channel_key: ChannelKey
    ) -> ! {
        // All participants that waited for this channel must be woke up
        // except current thread which goes to waiting state.
        let thread = self.thread.clone();
        let wake_up_list = self.schedw().network
                .channel_signal(&thread, &channel_key, true).unwrap();

        for thread_key in wake_up_list {
            let mut schedw = self.schedw();
            let thread_sched = schedw.waiting_threads.remove_key(&thread_key);
            schedw.threads.push(thread_sched);
        }

        self.timeout_interrupt()
    }

    /// Thread was waiting for the channel signal but timer has reached
    /// the timeout.
    pub fn channel_wait_timeout_interrupt(&mut self, key: ThreadKey,
        timer_remain: usize
    ) -> ! {
        // Update the network.
        self.schedw().network.sleep_thread(&key);

        // Move thread to the list of the threads that wait for processor time.
        // And add remaining time to thread.
        let mut thread_sched = self.schedw().waiting_threads.remove_key(&key);
        thread_sched.add_time += timer_remain as u32;
        self.schedw().threads.push(thread_sched);

        // Execute next thread.
        self.timeout_interrupt()
    }

    // Read access to scheduler.
    fn schedr(&self) -> RwLockReadGuard<Sched<C>> {
        self.sched.read().unwrap()
    }

    /// Read-write access to scheduler.
    fn schedw(&mut self) -> RwLockWriteGuard<Sched<C>> {
        self.sched.write().unwrap()
    }

    fn join(&self, mode: JoinMode) -> ! {
        // Use unsafe to get access to CPU and drop schedr so other cores
        // could access Sched.
        let cpu = unsafe {
            let ptr = &self.schedr().cpu as *const Box<C> as *mut Box<C>;
            &mut *ptr
        };
        cpu.join(mode)
    }
}

impl ThreadSched {

    pub fn key(&self) -> &ThreadKey {
        &self.key
    }
}

impl ThreadSet {

    /// Remove given thread key.
    pub fn remove_key(&mut self, key: &ThreadKey) -> ThreadSched {
        let pos = self.map.remove(key).unwrap();
        let last = if self.queue.len() == 1 {
            None
        } else {
            Some(self.queue.len() - 1)
        };

        if last.is_some() {
            let last = last.unwrap();
            let last_sched = self.queue.swap_remove(pos);

            // Update changed element position in the tree.
            let moved_element = self.map.get_mut(&last_sched.key).unwrap();
            *moved_element = pos;
            last_sched
        } else {
            self.queue.remove(pos)
        }
    }

    /// Add new thread scheduling information.
    pub fn add_key(&mut self, ts: ThreadSched) {
        let key = ts.key.clone();
        self.queue.push(ts);
        self.map.insert(key, self.queue.len() - 1);
    }
}

#[cfg(test)]
mod tests {
}
