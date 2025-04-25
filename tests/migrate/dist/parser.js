/* eslint-disable no-cond-assign */
const R_INDENT = /([ \t]*)/y;
const R_OPERATION = /([\w$_]+)\b\s*(:?)[ \t]*/y;
const R_BREAK = /-{3,}/y;
const R_LINE = /([^\n]*)/y;
const R_CLEAR = /(?:[ \t]*(?:;;[^\n]*)?\n)*/y;
const R_EOL = /\n+/y;
export class Parser {
    _s_program;
    _i_match = 0;
    _s_rest = '';
    _s_method = '';
    _s_sender = '';
    _nl_indent_method = 0;
    _nl_indent_sender = 0;
    constructor(_s_program) {
        this._s_program = _s_program;
        this._s_rest = _s_program;
    }
    _match(r_token, b_clear = false) {
        if (b_clear)
            this._clear();
        // set match index
        r_token.lastIndex = this._i_match;
        // attempt to match
        const m_token = r_token.exec(this._s_rest);
        // no match
        if (!m_token)
            return null;
        // succeeded; update match index
        this._i_match = r_token.lastIndex;
        // return matches
        return m_token;
    }
    _clear() {
        this._match(R_CLEAR);
    }
    args(s_method = this._s_method, s_sender = this._s_sender) {
        let m_next;
        if (m_next = this._match(R_LINE)) {
            const [, s_line] = m_next;
            const a_parts = s_line.trim().split(/\s+/g);
            // expects error
            let b_fail = false;
            let s_error = '';
            const i_error = a_parts.findIndex(s => s.startsWith('**fail'));
            if (i_error >= 0) {
                b_fail = true;
                s_error = a_parts.slice(i_error + 1).join(' ');
                a_parts.splice(i_error);
            }
            // debug
            let b_debug = false;
            const i_debug = a_parts.findIndex(s => s.startsWith('**debug'));
            if (i_debug >= 0) {
                b_debug = true;
                a_parts.splice(i_debug);
            }
            // compile statement
            return {
                type: 'statement',
                value: {
                    method: s_method,
                    sender: s_sender,
                    args: a_parts,
                    fail: b_fail,
                    debug: b_debug,
                    error: s_error,
                },
            };
        }
    }
    sender(s_indent, s_method = this._s_method) {
        let m_next;
        if (m_next = this._match(R_INDENT)) {
            const [, s_indent_local] = m_next;
            const nl_indent = (s_indent_local || s_indent).length;
            // sender defined and indented under; args
            if (this._s_sender && nl_indent > this._nl_indent_sender) {
                return this.args();
            }
            if (m_next = this._match(R_OPERATION, true)) {
                const [, s_sender, s_colon] = m_next;
                // clear after token
                if (s_colon)
                    this._match(R_CLEAR);
                // set/clear sender context
                this._s_sender = s_colon ? s_sender : '';
                this._nl_indent_sender = nl_indent;
                // evaluate args
                return this.args(s_method, s_sender);
            }
        }
    }
    statement() {
        let m_next;
        // clear preceding newlines
        this._clear();
        // match indent if any
        if (m_next = this._match(R_INDENT)) {
            const [, s_indent] = m_next;
            const nl_indent = s_indent.length;
            // sender defined and indented under; args
            if (this._s_sender && nl_indent > this._nl_indent_sender) {
                return this.args();
            }
            // method defined and indented under; sender
            else if (this._s_method && nl_indent > this._nl_indent_method) {
                return this.sender(s_indent);
            }
            // operation
            if (m_next = this._match(R_OPERATION)) {
                const [, s_method, s_colon] = m_next;
                // clear after token
                if (s_colon)
                    this._clear();
                // set/clear method context
                this._s_method = s_colon ? s_method : '';
                this._nl_indent_method = s_indent.length;
                // evaluate
                return this.sender(s_indent, s_method);
            }
            // break
            else if (this._match(R_BREAK)) {
                // clear context
                this._s_method = '';
                this._s_sender = '';
                // return break statement
                return {
                    type: 'break',
                };
            }
        }
    }
    *program() {
        const nl_program = this._s_program.length;
        while (-1 !== this._i_match && this._i_match < nl_program) {
            yield this.statement();
        }
    }
}
//# sourceMappingURL=parser.js.map