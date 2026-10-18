#pragma once

class Expression {
public:
    virtual int evaluate() const = 0;
};

int evaluate(const Expression&);
