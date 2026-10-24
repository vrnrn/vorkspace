//! AppKit owns menu navigation and accessibility; actions return to GPUI.
use objc2::runtime::AnyObject;
use objc2::runtime::NSObjectProtocol;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSFont, NSFontWeightSemibold, NSImage,
    NSImageView, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSTextField, NSView,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};
use std::rc::Rc;

const WIDTH: f64 = 310.;
const PORTRAIT_SIZE: f64 = 26.;
const IMAGE_X: f64 = 14.;
const TITLE_X: f64 = 48.;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MenuBot {
    pub id: String,
    pub name: String,
    pub mascot: String,
    pub color: u32,
}
struct Action {
    key: String,
    run: Rc<dyn Fn(&str)>,
}
define_class!(
    #[unsafe(super(NSMenuItem))]
    #[name="VorkspaceBotMenuItem"]
    #[thread_kind=MainThreadOnly]
    #[ivars=Action]
    struct Item;
    impl Item{
        #[unsafe(method(runBotAction:))]
        fn run(&self,_:Option<&AnyObject>){(self.ivars().run)(&self.ivars().key);}
    }
);
impl Item {
    fn new(
        mtm: MainThreadMarker,
        title: &str,
        key: String,
        run: Rc<dyn Fn(&str)>,
    ) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(Action { key, run });
        let item: Retained<Self> = unsafe {
            msg_send![super(this),initWithTitle:&*NSString::from_str(title),action:Some(sel!(runBotAction:)),keyEquivalent:&*NSString::from_str("")]
        };
        // The status menu retains each item for as long as it can be invoked.
        unsafe {
            item.setTarget(Some(&item));
        }
        item
    }
}

define_class!(
    /// A native menu item keeps AppKit's keyboard/accessibility behavior. Its
    /// view supplies the translucent hover pill used by the original menu.
    #[unsafe(super(NSView))]
    #[name = "VorkspaceBotStatusRow"]
    #[thread_kind = MainThreadOnly]
    struct MenuRow;

    impl MenuRow {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if !self.enclosingMenuItem().is_some_and(|item| item.isHighlighted()) {
                return;
            }
            let bounds = self.bounds();
            NSColor::labelColor().colorWithAlphaComponent(0.1).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSRect::new(
                    NSPoint::new(bounds.origin.x + 5., bounds.origin.y),
                    NSSize::new(bounds.size.width - 10., bounds.size.height),
                ),
                10., 10.,
            ).fill();
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: Option<&AnyObject>) {
            let Some(item) = self.enclosingMenuItem().filter(|item| item.isEnabled()) else { return; };
            let Some(menu) = (unsafe { item.menu() }) else { return; };
            menu.cancelTracking();
            menu.performActionForItemAtIndex(menu.indexOfItem(&item));
        }
    }
);

fn portrait(bot: &MenuBot) -> Option<Retained<NSImage>> {
    let bytes = super::avatar::menu_portrait_png(&bot.mascot, bot.color)?;
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes))?;
    image.setSize(NSSize::new(PORTRAIT_SIZE, PORTRAIT_SIZE));
    Some(image)
}

fn row(
    title: &str,
    image: Option<&NSImage>,
    bot: bool,
    enabled: bool,
    mtm: MainThreadMarker,
) -> Retained<NSView> {
    let height = if bot { 32. } else { 28. };
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height));
    let view: Retained<MenuRow> = unsafe { msg_send![MenuRow::alloc(mtm), initWithFrame: frame] };
    view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    if let Some(image) = image {
        let icon = NSImageView::imageViewWithImage(image, mtm);
        icon.setFrame(NSRect::new(
            NSPoint::new(IMAGE_X, (height - PORTRAIT_SIZE) / 2.),
            NSSize::new(PORTRAIT_SIZE, PORTRAIT_SIZE),
        ));
        view.addSubview(&icon);
        view.setToolTip(Some(&NSString::from_str("Open floating chat")));
    }
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(13.)));
    let color = if enabled {
        NSColor::labelColor()
    } else {
        NSColor::secondaryLabelColor()
    };
    label.setTextColor(Some(&color));
    label.sizeToFit();
    let title_x = if image.is_some() { TITLE_X } else { IMAGE_X };
    let label_height = label.frame().size.height;
    label.setFrame(NSRect::new(
        NSPoint::new(title_x, ((height - label_height) / 2.).round()),
        NSSize::new(WIDTH - title_x - 14., label_height),
    ));
    label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    view.addSubview(&label);
    view.into_super()
}

fn header(mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setEnabled(false);
    item.setTitle(&NSString::from_str("Bots"));
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, 34.)),
    );
    let label = NSTextField::labelWithString(&NSString::from_str("Bots"), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize_weight(13., unsafe {
        NSFontWeightSemibold
    })));
    label.setTextColor(Some(&NSColor::labelColor()));
    label.setFrame(NSRect::new(
        NSPoint::new(IMAGE_X, 9.),
        NSSize::new(WIDTH - 2. * IMAGE_X, 17.),
    ));
    label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    view.addSubview(&label);
    item.setView(Some(&view));
    item
}

pub(super) struct Status {
    item: Retained<NSStatusItem>,
    signature: Option<Vec<MenuBot>>,
}
impl Status {
    pub(super) fn new() -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
        if let Some(button) = item.button(mtm) {
            let data = NSData::with_bytes(include_bytes!("../../assets/menu-bar-icon.png"));
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                image.setSize(NSSize::new(18., 18.));
                image.setTemplate(false);
                button.setImage(Some(&image));
            } else {
                button.setTitle(&NSString::from_str("Bot"));
            }
            button.setToolTip(Some(&NSString::from_str("vorkspace")));
        }
        Some(Self {
            item,
            signature: None,
        })
    }
    pub(super) fn update(&mut self, bots: &[MenuBot], run: Rc<dyn Fn(&str)>) {
        if self.signature.as_deref() == Some(bots) {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setMinimumWidth(WIDTH);
        unsafe { menu.setFont(Some(&NSFont::systemFontOfSize(13.))) };
        menu.addItem(&header(mtm));
        if bots.is_empty() {
            let title = NSMenuItem::new(mtm);
            title.setTitle(&NSString::from_str("No bots yet"));
            title.setEnabled(false);
            title.setView(Some(&row("No bots yet", None, false, false, mtm)));
            menu.addItem(&title);
        }
        for bot in bots {
            let item = Item::new(mtm, &bot.name, bot.id.clone(), run.clone());
            let image = portrait(bot);
            item.setImage(image.as_deref());
            // macOS 27 hides menu images unless their visibility is explicit.
            if item.respondsToSelector(sel!(setPreferredImageVisibility:)) {
                let () = unsafe { msg_send![&item, setPreferredImageVisibility: 1isize] };
            }
            item.setView(Some(&row(&bot.name, image.as_deref(), true, true, mtm)));
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let composer = Item::new(mtm, "Quick Composer", "composer".into(), run.clone());
        composer.setView(Some(&row("Quick Composer", None, false, true, mtm)));
        menu.addItem(&composer);
        let quit = Item::new(mtm, "Quit vorkspace", "quit".into(), run);
        quit.setView(Some(&row("Quit vorkspace", None, false, true, mtm)));
        menu.addItem(&quit);
        self.item.setMenu(Some(&menu));
        self.signature = Some(bots.to_vec());
    }
}
impl Drop for Status {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
