//! Minimal DOM for tests: lets the XSS corpus re-parse our output with the
//! html5ever tree builder (the browser algorithm) and inspect the result.
//! ammonia's own rcdom is not public, and markup5ever_rcdom lags html5ever.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{parse_document, Attribute, ExpandedName, ParseOpts, QualName};

pub type Handle = Rc<Node>;

pub enum Data {
    Document,
    Other,
    Text(RefCell<StrTendril>),
    Element {
        name: QualName,
        attrs: RefCell<Vec<Attribute>>,
        template: Option<Handle>,
        annotation_xml_ip: bool,
    },
}

pub struct Node {
    pub data: Data,
    pub children: RefCell<Vec<Handle>>,
    parent: Cell<Option<Weak<Node>>>,
}

impl Node {
    fn new(data: Data) -> Handle {
        Rc::new(Node {
            data,
            children: RefCell::new(Vec::new()),
            parent: Cell::new(None),
        })
    }
}

impl Drop for Node {
    // Iterative drop so a 50k-deep tree doesn't overflow the stack.
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut *self.children.borrow_mut());
        while let Some(n) = stack.pop() {
            stack.extend(std::mem::take(&mut *n.children.borrow_mut()));
        }
    }
}

pub struct Dom {
    pub document: Handle,
}

fn detach(target: &Handle) {
    if let Some(parent) = target.parent.take().and_then(|w| w.upgrade()) {
        parent
            .children
            .borrow_mut()
            .retain(|c| !Rc::ptr_eq(c, target));
    }
}

fn append_node(parent: &Handle, child: Handle) {
    detach(&child);
    child.parent.set(Some(Rc::downgrade(parent)));
    parent.children.borrow_mut().push(child);
}

fn text_node(parent: &Handle, text: StrTendril) -> Option<Handle> {
    if let Some(last) = parent.children.borrow().last() {
        if let Data::Text(t) = &last.data {
            t.borrow_mut().push_tendril(&text);
            return None;
        }
    }
    Some(Node::new(Data::Text(RefCell::new(text))))
}

impl TreeSink for Dom {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }
    fn parse_error(&self, _msg: Cow<'static, str>) {}
    fn get_document(&self) -> Handle {
        self.document.clone()
    }
    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        match &target.data {
            Data::Element { name, .. } => name.expanded(),
            _ => panic!("not an element"),
        }
    }
    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        Node::new(Data::Element {
            name,
            attrs: RefCell::new(attrs),
            template: flags.template.then(|| Node::new(Data::Document)),
            annotation_xml_ip: flags.mathml_annotation_xml_integration_point,
        })
    }
    fn create_comment(&self, _text: StrTendril) -> Handle {
        Node::new(Data::Other)
    }
    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Handle {
        Node::new(Data::Other)
    }
    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        match child {
            NodeOrText::AppendNode(n) => append_node(parent, n),
            NodeOrText::AppendText(t) => {
                if let Some(n) = text_node(parent, t) {
                    append_node(parent, n)
                }
            }
        }
    }
    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev: &Handle,
        child: NodeOrText<Handle>,
    ) {
        let parent = element.parent.take();
        let has_parent = parent.as_ref().is_some_and(|w| w.upgrade().is_some());
        element.parent.set(parent);
        if has_parent {
            self.append_before_sibling(element, child)
        } else {
            self.append(prev, child)
        }
    }
    fn append_doctype_to_document(&self, _n: StrTendril, _p: StrTendril, _s: StrTendril) {}
    fn get_template_contents(&self, target: &Handle) -> Handle {
        match &target.data {
            Data::Element {
                template: Some(t), ..
            } => t.clone(),
            _ => panic!("not a template"),
        }
    }
    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        Rc::ptr_eq(x, y)
    }
    fn set_quirks_mode(&self, _mode: QuirksMode) {}
    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        let parent = sibling
            .parent
            .take()
            .and_then(|w| w.upgrade())
            .expect("sibling without parent");
        sibling.parent.set(Some(Rc::downgrade(&parent)));
        let i = parent
            .children
            .borrow()
            .iter()
            .position(|c| Rc::ptr_eq(c, sibling))
            .unwrap();
        let node = match child {
            NodeOrText::AppendNode(n) => n,
            NodeOrText::AppendText(t) => {
                if i > 0 {
                    if let Data::Text(prev) = &parent.children.borrow()[i - 1].data {
                        prev.borrow_mut().push_tendril(&t);
                        return;
                    }
                }
                Node::new(Data::Text(RefCell::new(t)))
            }
        };
        detach(&node);
        let i = parent
            .children
            .borrow()
            .iter()
            .position(|c| Rc::ptr_eq(c, sibling))
            .unwrap();
        node.parent.set(Some(Rc::downgrade(&parent)));
        parent.children.borrow_mut().insert(i, node);
    }
    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if let Data::Element {
            attrs: existing, ..
        } = &target.data
        {
            let mut existing = existing.borrow_mut();
            let names: HashSet<QualName> = existing.iter().map(|a| a.name.clone()).collect();
            existing.extend(attrs.into_iter().filter(|a| !names.contains(&a.name)));
        }
    }
    fn remove_from_parent(&self, target: &Handle) {
        detach(target);
    }
    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        let children = std::mem::take(&mut *node.children.borrow_mut());
        for c in children {
            c.parent.set(Some(Rc::downgrade(new_parent)));
            new_parent.children.borrow_mut().push(c);
        }
    }
    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        matches!(
            &target.data,
            Data::Element {
                annotation_xml_ip: true,
                ..
            }
        )
    }
}

pub fn parse(html: &str) -> Dom {
    parse_document(
        Dom {
            document: Node::new(Data::Document),
        },
        ParseOpts::default(),
    )
    .one(html)
}
