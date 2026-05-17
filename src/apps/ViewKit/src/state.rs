use std::sync::{Arc, Mutex};
use std::fmt;

/// リアクティブな状態を管理するジェネリック型
#[derive(Clone)]
pub struct State<T: Clone + Send + Sync + 'static> {
    inner: Arc<Mutex<T>>,
    listeners: Arc<Mutex<Vec<Box<dyn Fn() + Send + Sync>>>>,
}

impl<T: Clone + Send + Sync + 'static> State<T> {
    /// 新しい State を作成
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(value)),
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 現在の値を取得
    pub fn get(&self) -> T {
        self.inner.lock().unwrap().clone()
    }

    /// 値をセットして、リスナーに通知
    pub fn set(&self, value: T) {
        {
            let mut inner = self.inner.lock().unwrap();
            *inner = value;
        }
        self.notify();
    }

    /// 値を更新関数で変更
    pub fn update(&self, f: impl Fn(T) -> T) {
        {
            let mut inner = self.inner.lock().unwrap();
            *inner = f(inner.clone());
        }
        self.notify();
    }

    /// リスナーを登録
    pub fn on_change(&self, listener: Box<dyn Fn() + Send + Sync>) {
        let mut listeners = self.listeners.lock().unwrap();
        listeners.push(listener);
    }

    /// 全リスナーに通知
    fn notify(&self) {
        let listeners = self.listeners.lock().unwrap();
        for listener in listeners.iter() {
            listener();
        }
    }
}

impl<T: Clone + Send + Sync + fmt::Debug + 'static> fmt::Debug for State<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("value", &self.get())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_state_creation_and_get() {
        let state = State::new(42);
        assert_eq!(state.get(), 42);
    }

    #[test]
    fn test_state_set() {
        let state = State::new(42);
        state.set(100);
        assert_eq!(state.get(), 100);
    }

    #[test]
    fn test_state_update() {
        let state = State::new(10);
        state.update(|v| v + 5);
        assert_eq!(state.get(), 15);
    }

    #[test]
    fn test_state_listener() {
        let state = State::new(0);
        let triggered = Arc::new(AtomicBool::new(false));
        let triggered_clone = triggered.clone();

        state.on_change(Box::new(move || {
            triggered_clone.store(true, Ordering::SeqCst);
        }));

        state.set(1);
        assert!(triggered.load(Ordering::SeqCst));
    }
}
