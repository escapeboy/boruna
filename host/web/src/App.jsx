import React, { useState, useCallback } from 'react'
import { renderUiTree } from './renderer.jsx'

/**
 * Boruna Web Host Application.
 *
 * This host receives declarative UI trees (JSON) from the runtime
 * and renders them using React. All business logic lives in bytecode;
 * this is purely a rendering layer.
 *
 * Communication flow:
 *   Runtime → UI tree (JSON) → Host renders
 *   User interaction → Event (JSON) → Runtime message
 */
export function App() {
  const [uiTrees, setUiTrees] = useState([])
  const [result, setResult] = useState(null)
  const [error, setError] = useState(null)
  const [loading, setLoading] = useState(false)

  const runProgram = useCallback(async (file) => {
    setLoading(true)
    setError(null)
    try {
      // In production, this would communicate with the runtime via IPC/WebSocket.
      // For now, we load a pre-computed UI output JSON file.
      const response = await fetch(`/api/run?file=${encodeURIComponent(file)}`)
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      const data = await response.json()
      setUiTrees(data.ui_output || [])
      setResult(data.result)
    } catch (e) {
      setError(e.message)
    } finally {
      setLoading(false)
    }
  }, [])

  const sendEvent = useCallback(async (event) => {
    try {
      const response = await fetch('/api/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(event),
      })
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      const data = await response.json()
      setUiTrees(data.ui_output || [])
      setResult(data.result)
    } catch (e) {
      setError(e.message)
    }
  }, [])

  return (
    <div style={{ fontFamily: 'system-ui', maxWidth: 800, margin: '0 auto', padding: 20 }}>
      <h1>Boruna Web Host</h1>

      <div style={{ marginBottom: 20 }}>
        <button onClick={() => runProgram('examples/counter.ax')}>
          Run Counter
        </button>
        <button onClick={() => runProgram('examples/todo.ax')} style={{ marginLeft: 8 }}>
          Run Todo
        </button>
      </div>

      {loading && <p>Running...</p>}
      {error && <p style={{ color: 'red' }}>Error: {error}</p>}

      {result != null && (
        <div style={{ background: '#f0f0f0', padding: 12, borderRadius: 4, marginBottom: 16 }}>
          <strong>Result:</strong> <code>{JSON.stringify(result)}</code>
        </div>
      )}

      {uiTrees.length > 0 && (
        <div>
          <h2>UI Output</h2>
          {uiTrees.map((tree, i) => (
            <div key={i} style={{ marginBottom: 12, padding: 12, border: '1px solid #ddd', borderRadius: 4 }}>
              {renderUiTree(tree, sendEvent)}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
