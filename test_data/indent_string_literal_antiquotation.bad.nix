{
  foo = ''
    bar = ${builtins.concatStringsSep " " [
1
2
3
]}
    bla = hoi
  '';

bar = ''
foo
${
foo
}
foo
  '';

baz =
    ''
foo
${
foo
}
foo
    '';

qux =
    ''
    bar = ${builtins.concatStringsSep " " [
1
2
3
]}
    bla = hoi
    '';
}
