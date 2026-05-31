//! The `Frame`: everything the meta-crate produces from one
//! render/script call.

use boa_cat::Value;
use boa_cat::heap::Heap;
use dom_cat::Document as DomDocument;
use layout_cat::LayoutTree;
use paint_cat::DisplayList;

/// A complete frame.  Carries everything downstream code needs to
/// paint the page or inspect the post-script state.
#[derive(Debug, Clone)]
pub struct Frame {
    document: DomDocument,
    layout_tree: LayoutTree,
    display_list: DisplayList,
    script_value: Value,
    script_heap: Heap,
}

impl Frame {
    /// Build a frame.
    #[must_use]
    pub fn new(
        document: DomDocument,
        layout_tree: LayoutTree,
        display_list: DisplayList,
        script_value: Value,
        script_heap: Heap,
    ) -> Self {
        Self {
            document,
            layout_tree,
            display_list,
            script_value,
            script_heap,
        }
    }

    /// The parsed and (when scripted) mutated DOM.
    #[must_use]
    pub fn document(&self) -> &DomDocument {
        &self.document
    }

    /// The computed layout tree.
    #[must_use]
    pub fn layout_tree(&self) -> &LayoutTree {
        &self.layout_tree
    }

    /// The paint-cat display list, ready for a backend renderer.
    #[must_use]
    pub fn display_list(&self) -> &DisplayList {
        &self.display_list
    }

    /// The final JS value (last-expression value of the script).
    /// `Value::Undefined` for `render()` calls (no script).
    #[must_use]
    pub fn script_value(&self) -> &Value {
        &self.script_value
    }

    /// The boa-cat heap after script execution.  Useful for inspecting
    /// post-script JS state (e.g. mutated element-objects).
    /// Empty for `render()` calls.
    #[must_use]
    pub fn script_heap(&self) -> &Heap {
        &self.script_heap
    }
}
