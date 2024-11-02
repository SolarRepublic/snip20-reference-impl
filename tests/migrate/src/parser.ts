/* eslint-disable no-cond-assign */



// const R_STATEMENT = /^\s*(\w+)\b/;
const R_INDENT = /([ \t]*)/y;
const R_OPERATION = /(\w+)\b\s*(:?)[ \t]*/y;
const R_LINE = /([^\n]*)/y;
const R_CLEAR = /(?:[ \t]*(?:;;[^\n]*)?\n)*/y;

type Match = RegExpExecArray | null;

type Statement = {
	method: string;
	sender: string;
	args: string[];
};

type StatementResult = Statement | undefined | void;

export class Parser {
	protected _i_match = 0;
	protected _s_rest = '';
	protected _s_method = '';
	protected _s_sender = '';

	protected _s_indent_method = '';
	protected _s_indent_sender = '';

	constructor(protected _s_program: string) {
		this._s_rest = _s_program;
	}

	_match(r_token: RegExp, b_clear=false): Match {
		if(b_clear) this._clear();

		// set match index
		r_token.lastIndex = this._i_match;

		// attempt to match
		const m_token = r_token.exec(this._s_rest);

		// no match
		if(!m_token) return null;

		// succeeded; update match index
		this._i_match = r_token.lastIndex;

		// return matches
		return m_token;
	}

	_clear(): void {
		this._match(R_CLEAR);
	}

	args(s_method=this._s_method, s_sender=this._s_sender): StatementResult {
		let m_next: Match;

		if(m_next=this._match(R_LINE)) {
			this._clear();

			const [, s_line] = m_next;

			const a_parts = s_line.trim().split(/\s+/g);

			// compile statement
			return {
				method: s_method,
				sender: s_sender,
				args: a_parts,
			};
		}
	}

	sender(s_method=this._s_method): StatementResult {
		let m_next: Match;

		if(m_next=this._match(R_INDENT)) {
			const [, s_indent] = m_next;

			const nl_indent = s_indent.length;

			// sender defined and indented under; args
			if(this._s_sender && nl_indent > this._s_indent_sender.length) {
				return this.args();
			}

			if(m_next=this._match(R_OPERATION, true)) {
				const [, s_sender, s_colon] = m_next;

				// clear after token
				if(s_colon) this._match(R_CLEAR);

				// set/clear sender context
				this._s_sender = s_colon? s_sender: '';
				this._s_indent_sender = s_indent;

				// evaluate args
				return this.args(s_method, s_sender);
			}
		}
	}

	statement(): StatementResult {
		let m_next: Match;

		if(m_next=this._match(R_INDENT)) {
			const [, s_indent] = m_next;

			const nl_indent = s_indent.length;

			// sender defined and indented under; args
			if(this._s_sender && nl_indent > this._s_indent_sender.length) {
				return this.args();
			}
			// method defined and indented under; sender
			else if(this._s_method && nl_indent > this._s_indent_method.length) {
				return this.sender();
			}

			if(m_next=this._match(R_OPERATION, true)) {
				const [, s_method, s_colon] = m_next;

				// clear after token
				if(s_colon) this._match(R_CLEAR);

				// set/clear method context
				this._s_method = s_colon? s_method: '';
				this._s_indent_method = s_indent;

				// evaluate
				return this.sender(s_method);
			}
		}
	}

	* program(): Generator<StatementResult, void, unknown> {
		this._clear();

		const nl_program = this._s_program.length;

		while(-1 !== this._i_match && this._i_match < nl_program) {
			yield this.statement();
		}
	}
}

