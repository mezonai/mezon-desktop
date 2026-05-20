use gpui::{App, AppContext, Context, Entity, Subscription};

/// Stores lifecycle subscriptions owned by a view.
#[derive(Default)]
pub struct LifecycleSubscriptions {
    subscriptions: Vec<Subscription>,
}

impl LifecycleSubscriptions {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, subscription: Subscription) {
        self.subscriptions.push(subscription);
    }
}

/// Optional entity lifecycle hooks for GPUI views.
///
/// `on_init` runs once while the entity is being created. `on_destroy` runs when
/// GPUI releases the entity, not when a route merely stops rendering it.
pub trait ViewLifecycle: Sized + 'static {
    fn lifecycle_subscriptions(&mut self) -> &mut LifecycleSubscriptions;

    fn on_init(&mut self, _cx: &mut Context<Self>) {}

    fn on_destroy(&mut self, _cx: &mut App) {}
}

pub trait ViewLifecycleContext: AppContext {
    fn new_lifecycle_view<T: ViewLifecycle>(
        &mut self,
        build_view: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T> {
        self.new(|cx| {
            let mut view = build_view(cx);
            view.on_init(cx);

            let release = cx.on_release(|view, cx| {
                view.on_destroy(cx);
            });
            view.lifecycle_subscriptions().push(release);

            view
        })
    }
}

impl<T: AppContext> ViewLifecycleContext for T {}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::TestAppContext;

    use super::*;

    #[derive(Default)]
    struct LifecycleProbe {
        lifecycle: LifecycleSubscriptions,
        init_count: Rc<Cell<usize>>,
        destroy_count: Rc<Cell<usize>>,
    }

    impl LifecycleProbe {
        fn new(
            init_count: Rc<Cell<usize>>,
            destroy_count: Rc<Cell<usize>>,
            _cx: &mut Context<Self>,
        ) -> Self {
            Self {
                lifecycle: LifecycleSubscriptions::new(),
                init_count,
                destroy_count,
            }
        }
    }

    impl ViewLifecycle for LifecycleProbe {
        fn lifecycle_subscriptions(&mut self) -> &mut LifecycleSubscriptions {
            &mut self.lifecycle
        }

        fn on_init(&mut self, _cx: &mut Context<Self>) {
            self.init_count.set(self.init_count.get() + 1);
        }

        fn on_destroy(&mut self, _cx: &mut App) {
            self.destroy_count.set(self.destroy_count.get() + 1);
        }
    }

    #[gpui::test]
    fn runs_init_once_when_created(cx: &mut TestAppContext) {
        let init_count = Rc::new(Cell::new(0));
        let destroy_count = Rc::new(Cell::new(0));

        let entity = cx.update(|cx| {
            cx.new_lifecycle_view({
                let init_count = init_count.clone();
                let destroy_count = destroy_count.clone();
                move |cx| LifecycleProbe::new(init_count, destroy_count, cx)
            })
        });

        assert_eq!(init_count.get(), 1);
        entity.update(cx, |_, cx| cx.notify());
        assert_eq!(init_count.get(), 1);
        assert_eq!(destroy_count.get(), 0);
    }

    #[gpui::test]
    fn runs_destroy_when_entity_is_released(cx: &mut TestAppContext) {
        let init_count = Rc::new(Cell::new(0));
        let destroy_count = Rc::new(Cell::new(0));

        let weak = {
            let entity = cx.update(|cx| {
                cx.new_lifecycle_view({
                    let init_count = init_count.clone();
                    let destroy_count = destroy_count.clone();
                    move |cx| LifecycleProbe::new(init_count, destroy_count, cx)
                })
            });
            let weak = entity.downgrade();
            drop(entity);
            weak
        };

        cx.update(|_| {});

        assert_eq!(init_count.get(), 1);
        assert_eq!(destroy_count.get(), 1);
        weak.assert_released();
    }
}
