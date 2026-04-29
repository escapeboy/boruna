# std-notifications

> Notification queue and toast helpers

**Package:** `std.notifications`  **Version:** `0.1.0`  **Capabilities required:** `time.now`

## Overview

`std-notifications` manages a bounded in-memory queue of toast-style notifications. Add messages with the level-specific helpers (`notification_success`, `notification_error`, etc.), dismiss them by ID, and schedule auto-dismiss timers with `notification_dismiss_effect`. All queue management is pure; only auto-dismiss produces a timer `Effect`.

## Installation

Add to your `package.ax.json` dependencies:

```json
"std.notifications": "0.1.0"
```

Your policy must grant `time.now` if you use `notification_dismiss_effect`.

## API Reference

### Types

#### `Effect`

```
type Effect { kind: String, payload: String, callback_tag: String }
```

#### `Notification`

```
type Notification { id: Int, level: String, message: String, dismiss_ms: Int }
```

- `id` — unique monotonic integer assigned at push time
- `level` — `"info"`, `"success"`, `"warning"`, or `"error"`
- `dismiss_ms` — milliseconds until auto-dismiss; `0` means no auto-dismiss

#### `NotificationQueue`

```
type NotificationQueue {
    next_id: Int,
    count: Int,
    max_visible: Int,
    last_level: String,
    last_message: String,
    last_id: Int
}
```

- `next_id` — the ID that will be assigned to the next pushed notification
- `count` — number of currently visible notifications
- `max_visible` — cap beyond which new pushes should be blocked or the oldest dropped
- `last_level` / `last_message` / `last_id` — fields of the most recently pushed notification

### Functions

#### Initialization

##### `notification_init(max_visible: Int) -> NotificationQueue`

Creates an empty queue with the given visible-notification cap.

#### Pushing notifications

##### `notification_info(queue: NotificationQueue, level: String, message: String) -> NotificationQueue`

Pushes a notification with level `"info"` and `dismiss_ms: 3000`.

##### `notification_success(queue: NotificationQueue, message: String) -> NotificationQueue`

Pushes a success notification with `dismiss_ms: 3000`.

##### `notification_warning(queue: NotificationQueue, message: String) -> NotificationQueue`

Pushes a warning notification with `dismiss_ms: 5000`.

##### `notification_error(queue: NotificationQueue, message: String) -> NotificationQueue`

Pushes an error notification with `dismiss_ms: 0` (sticky — does not auto-dismiss).

##### `notification_push(queue: NotificationQueue, level: String, message: String, dismiss_ms: Int) -> NotificationQueue`

Low-level push with explicit level and dismiss duration.

**Example**
```
fn main() -> Int {
  let q: NotificationQueue = notification_init(5)
  let q1: NotificationQueue = notification_success(q, "Changes saved")
  let q2: NotificationQueue = notification_error(q1, "Upload failed")
  q2.count
}
```

#### Constructing a notification record

##### `notification_make(id: Int, level: String, message: String, dismiss_ms: Int) -> Notification`

Constructs a `Notification` value directly. Use when rendering the queue in your `view`.

#### Dismissing

##### `notification_dismiss(queue: NotificationQueue, id: Int) -> NotificationQueue`

Decrements `count` by one. The `id` parameter is informational; the implementation does not track individual items in the current version.

##### `notification_dismiss_effect(id: Int, delay_ms: Int) -> Effect`

Produces a timer effect that fires after `delay_ms` milliseconds. Wire the `"notification_dismissed"` callback to call `notification_dismiss` in your `update` handler.

#### Predicates

##### `notification_count(queue: NotificationQueue) -> Int`

Returns the current number of visible notifications.

##### `notification_is_full(queue: NotificationQueue) -> Int`

Returns `1` if `count >= max_visible`.

## Capabilities

`time.now` is required for the timer effect produced by `notification_dismiss_effect`. Queue-management functions are pure and need no capability.

## Notes / Limitations

- The queue does not store individual `Notification` records internally. `last_id`, `last_level`, and `last_message` reflect only the most recently pushed item. To render all visible notifications your app state should maintain a list of `Notification` values alongside the `NotificationQueue`.
- `notification_dismiss` decrements the count without validating the `id`. Calling it more times than there are visible notifications will clamp at `0`.
- Auto-dismiss (`dismiss_ms > 0`) requires your `update` handler to return the timer effect and handle the `"notification_dismissed"` callback.
