use boruna_bytecode::Value;

/// A node in the declarative UI tree.
///
/// UITree is produced by the view() function and rendered by the host.
/// The framework does not render anything — it only passes the tree through.
#[derive(Debug, Clone)]
pub struct UINode {
    pub tag: String,
    pub props: Vec<(String, Value)>,
    pub children: Vec<UINode>,
}

impl UINode {
    pub fn new(tag: impl Into<String>) -> Self {
        UINode {
            tag: tag.into(),
            props: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_prop(mut self, key: impl Into<String>, value: Value) -> Self {
        self.props.push((key.into(), value));
        self
    }

    pub fn with_child(mut self, child: UINode) -> Self {
        self.children.push(child);
        self
    }
}

/// Convert a VM Value (produced by view()) into a UINode tree.
///
/// Expected structure: Record with fields [tag, props_json, children_json]
/// or any Value that the host can render directly.
pub fn value_to_ui_tree(value: &Value) -> UINode {
    match value {
        Value::Record { fields, .. } => {
            let tag = match fields.first() {
                Some(Value::String(s)) => s.clone(),
                _ => "div".into(),
            };
            // props and children are encoded as nested values
            let mut node = UINode::new(tag);
            // Remaining fields become props
            for (i, field) in fields.iter().enumerate().skip(1) {
                node.props.push((format!("field_{i}"), field.clone()));
            }
            node
        }
        Value::String(s) => UINode::new("text").with_prop("value", Value::String(s.clone())),
        Value::Int(n) => UINode::new("text").with_prop("value", Value::Int(*n)),
        Value::Bool(b) => UINode::new("text").with_prop("value", Value::Bool(*b)),
        Value::List(items) => {
            let mut node = UINode::new("list");
            for item in items {
                node.children.push(value_to_ui_tree(item));
            }
            node
        }
        _ => UINode::new("raw").with_prop("value", value.clone()),
    }
}

/// Convert a UINode tree back to a Value for serialization.
pub fn ui_tree_to_value(node: &UINode) -> Value {
    let mut fields = vec![Value::String(node.tag.clone())];

    // Props as a map-like record
    let prop_values: Vec<Value> = node.props.iter()
        .map(|(_, v)| v.clone())
        .collect();
    fields.push(Value::List(prop_values));

    // Children
    let child_values: Vec<Value> = node.children.iter()
        .map(ui_tree_to_value)
        .collect();
    fields.push(Value::List(child_values));

    Value::Record { type_id: 0, fields }
}
