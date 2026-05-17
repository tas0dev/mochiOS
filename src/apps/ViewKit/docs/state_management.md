---
title: "State Management in ViewKit"
file: "example/stateful_ui.rs"
---

# State Management in ViewKit

ViewKit provides a reactive state management system that allows you to manage UI state and automatically update the UI in response to state changes.

## Basic Usage

### Creating State<T>

```rust
use viewkit::State;
use std::sync::Arc;

// Create with an initial value
let count: Arc<State<i32>> = Arc::new(State::new(0));
```

### Getting and Setting Values

```rust
// Get the current value
let current = count.get();  // 0

// Set a value
count.set(1);

// Use an update function
count.update(|v| v + 1);
```

### Combining with Event Handlers

```rust
let count = Arc::new(State::new(0));

button()
    .label("Increment")
    .on_click({
        let count = count.clone();
        move || {
            count.update(|v| v + 1);
            println!("Count: {}", count.get());
        }
    })
```

### Conditional Rendering

```rust
let is_logged_in = Arc::new(State::new(false));

let login_button = button()
    .label("Login")
    .if_visible(!is_logged_in.get());

let logout_button = button()
    .label("Logout")
    .if_visible(is_logged_in.get());
```

## Usage Example: Screen Navigation

```rust
// Manage screen state
let screen_state: Arc<State<i32>> = Arc::new(State::new(0));  // 0: Home, 1: Detail

// Home screen
let home_screen = {
    let state = screen_state.clone();
    card()
        .label("Home")
        .on_click(move || {
            state.set(1);  // Navigate to detail screen
        })
};

// Detail screen
let detail_screen = {
    let state = screen_state.clone();
    card()
        .label("Detail")
        .on_click(move || {
            state.set(0);  // Return to home screen
        })
};

// Build UI based on current state
let ui = if screen_state.get() == 0 {
    home_screen
} else {
    detail_screen
};
```

## Registering Listeners

You can monitor state changes and execute custom logic:

```rust
let state = State::new(0);

state.on_change(Box::new(|| {
    println!("State changed!");
}));

state.set(1);  // Prints "State changed!"
```

## Best Practices

1. **Multiple States**: Group related state and manage with composite types
```rust
   #[derive(Clone)]
   struct AppState {
       current_screen: i32,
       is_loading: bool,
       selected_item: Option<String>,
   }
   let state = Arc::new(State::new(AppState { ... }));
```

2. **Scope Management**: Be explicit when cloning
```rust
   let state = screen_state.clone();  // Explicitly clone
   move || {
       state.set(1);  // Use the cloned state
   }
```

3. **Maintaining Immutability**: Values in State must be Clone & Send + Sync
```rust
   // OK: Primitive types like String, i32, bool
   let state = State::new("Hello".to_string());
   
   // OK: Standard containers like Vec, HashMap
   let state = State::new(vec![1, 2, 3]);
   
   // NG: Types with interior mutability (Rc, RefCell, etc.)
   // let state = State::new(Rc::new(...));  // Compile error
```