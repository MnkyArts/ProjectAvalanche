use bevy::tasks::{ComputeTaskPool, Task, TaskPool};

type Joined<S, O> = std::thread::Result<(Box<S>, O)>;

pub struct FrameTask<S: Send + 'static, O: Send + 'static> {
    state: Option<Box<S>>,
    task: Option<Task<Joined<S, O>>>,
    done: Option<O>,
}

impl<S: Send + 'static, O: Send + 'static> FrameTask<S, O> {
    pub fn new(state: S) -> Self {
        FrameTask { state: Some(Box::new(state)), task: None, done: None }
    }

    pub fn settle(&mut self) -> Option<&mut S> {
        if let Some(task) = self.task.take() {
            match bevy::tasks::block_on(task) {
                Ok((state, out)) => {
                    self.state = Some(state);
                    self.done = Some(out);
                }
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        self.state.as_deref_mut()
    }

    pub fn spawn(&mut self, work: impl FnOnce(&mut S) -> O + Send + 'static) {
        self.settle();
        let Some(mut state) = self.state.take() else { return };
        let pool = ComputeTaskPool::get_or_init(TaskPool::default);
        self.task = Some(pool.spawn(async move {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let out = work(&mut state);
                (state, out)
            }))
        }));
    }

    pub fn pending(&self) -> bool {
        self.task.is_some() || self.done.is_some()
    }

    pub fn take_done(&mut self) -> Option<O> {
        self.done.take()
    }
}
