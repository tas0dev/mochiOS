---
title: "How to create components?"
file: "resources/components/*"
---

# How to create components?

Components are basically created using HTML and CSS.
The location where they are created is not yet defined.

Components must not have the `body`, `html`, or `head` tag specified.

Doing so will cause them to malfunction.

When specifying styles, write the `style` tag **at the beginning of the file** and define the styles there.

When using tags such as `div`, be sure to specify a unique class using `class`.

Then, CSS should only be applied to that class.

Currently, there are two special tags (used only in ViewKit) used when creating components:

- `<Children />`
- `<Content />`

Use `<Children />` to nest other components, and `<Content />` to insert raw data like text or images.

We will explain each in detail.

### Children tag
The Children tag primarily inserts the specified child elements into the location where it is written.

This is a frequently used tag, essential when displaying buttons in a Dock or placing text in a card.

### Content tag
The Content tag inserts data other than components, such as strings and images.
You can specify the content that can be inserted as Content. For example, to specify a string, you would write:
```html
<Content type="String" />
```

To insert an image, write:
```html
<Content type="Image" />
```

You can also add attributes to the Content tag to control how the image is rendered. For example, to make an image fill the component area while keeping its aspect ratio, use:
```html
<Content type="Image" fit="cover" clip-radius="15" />
```

`fit="cover"` makes the image crop to the component's bounds, and `clip-radius` applies rounded corners to the clipped image.

### Example
```html
<style>
    .card {
        border-radius: 10px;
        width: fit-content;
        height: fit-content;
        background-color: #fdfdfd;
        opacity: 100%;
        margin: 20px;
        padding: 20px
    }
</style>

<div class="card">
    <Children />
</div>
```

### Size Placeholders
In the CSS section, you can use special placeholders to dynamically set the size of your component. These are replaced by the properties assigned to the component at runtime:

`CONTENT_W`: Replaced by the value specified in the component's `.width` property.
`CONTENT_H`: Replaced by the value specified in the component's `.height` property.

In ViewKit, you can set these with `.width()` and `.height()` on `VComponent`.

Example:

```html
<style>
.component {
    /* each is replaced at runtime. */
    width: CONTENT_W;
    height: CONTENT_H;
}
</style>
<div class="component">
    <Children />
</div>
```

```rust
fn main() -> Result<(), String> {
    const WIDTH: u32 = 960;
    const HEIGHT: u32 = 540;

    AppBuilder::new(WIDTH, HEIGHT)
        .children(|| {
            // width() -> CONTENT_W
            // height() -> CONTENT_H
            card().width(WIDTH).height(HEIGHT)
        })?
        .build()?
        .run()
}
```
