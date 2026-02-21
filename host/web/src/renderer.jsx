import React from 'react'

/**
 * Render a declarative UI tree from the Boruna runtime.
 *
 * The runtime emits JSON values via the EmitUi instruction.
 * This renderer interprets those values as UI components.
 *
 * Supported value shapes:
 * - Record: renders as a card with fields
 * - String: renders as text
 * - Int/Float: renders as number
 * - Bool: renders as checkbox
 * - List: renders as list
 * - Map: renders as key-value table
 */
export function renderUiTree(tree, onEvent) {
  if (tree == null) return null

  // Handle tagged value types from the VM
  if (typeof tree === 'object') {
    // Record type
    if (tree.Record) {
      return renderRecord(tree.Record, onEvent)
    }
    // Enum type
    if (tree.Enum) {
      return renderEnum(tree.Enum, onEvent)
    }
    // List
    if (tree.List) {
      return (
        <ul>
          {tree.List.map((item, i) => (
            <li key={i}>{renderUiTree(item, onEvent)}</li>
          ))}
        </ul>
      )
    }
    // Map
    if (tree.Map) {
      return (
        <table style={{ borderCollapse: 'collapse' }}>
          <tbody>
            {Object.entries(tree.Map).map(([k, v]) => (
              <tr key={k}>
                <td style={{ border: '1px solid #ccc', padding: 4, fontWeight: 'bold' }}>{k}</td>
                <td style={{ border: '1px solid #ccc', padding: 4 }}>{renderUiTree(v, onEvent)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )
    }
    // Primitive wrappers
    if ('Int' in tree) return <span>{tree.Int}</span>
    if ('Float' in tree) return <span>{tree.Float}</span>
    if ('Bool' in tree) return <span>{tree.Bool ? 'true' : 'false'}</span>
    if ('String' in tree) return <span>{tree.String}</span>
    if (tree.Unit !== undefined) return <span>(unit)</span>
    if (tree.None !== undefined) return <span style={{ color: '#999' }}>None</span>
    if (tree.Some) return renderUiTree(tree.Some, onEvent)
  }

  // Fallback: render as JSON
  return <pre>{JSON.stringify(tree, null, 2)}</pre>
}

function renderRecord(record, onEvent) {
  const { type_id, fields } = record
  return (
    <div style={{ background: '#fafafa', padding: 8, borderRadius: 4 }}>
      <div style={{ fontSize: 12, color: '#666', marginBottom: 4 }}>
        Record #{type_id}
      </div>
      {fields.map((field, i) => (
        <div key={i} style={{ marginLeft: 12 }}>
          <span style={{ color: '#888' }}>field_{i}:</span>{' '}
          {renderUiTree(field, onEvent)}
        </div>
      ))}
    </div>
  )
}

function renderEnum(enumVal, onEvent) {
  const { type_id, variant, payload } = enumVal
  return (
    <div style={{ background: '#f0f8ff', padding: 8, borderRadius: 4 }}>
      <span style={{ fontSize: 12, color: '#666' }}>
        Enum #{type_id}::variant_{variant}
      </span>
      <div style={{ marginLeft: 12 }}>
        {renderUiTree(payload, onEvent)}
      </div>
    </div>
  )
}
